// Package testnet creates an isolated, in-process tailnet for integration tests.
// All persistent tsnet state lives in testing.TB.TempDir directories.
package testnet

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"remotescreen/tailnet/internal/testcontrol"
	"tailscale.com/envknob"
	"tailscale.com/ipn/store/mem"
	"tailscale.com/logtail"
	"tailscale.com/net/netns"
	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
	"tailscale.com/tstest/integration"
	"tailscale.com/types/logger"
)

type ControlOptions struct {
	RequireAuth bool
	AuthKey     string
}

type Control struct {
	URL      string
	AdminURL string
	AuthKey  string
	Server   *testcontrol.Server
}

var setupOnce sync.Once

func StartControl(t testing.TB, o ControlOptions) *Control {
	t.Helper()
	setupOnce.Do(func() {
		logtail.Disable()
		envknob.SetNoLogsNoSupport()
		envknob.Setenv("TS_DISABLE_PORTMAPPER", "true")
		netns.SetEnabled(false)
	})
	dm := integration.RunDERPAndSTUN(t, logger.Discard, "127.0.0.1")
	s := &testcontrol.Server{
		DERPMap: dm, DNSConfig: &tailcfg.DNSConfig{Proxied: true},
		MagicDNSDomain: "rs.test", RequireAuth: o.RequireAuth, RequireAuthKey: o.AuthKey,
		Logf: logger.Discard,
	}
	s.HTTPTestServer = httptest.NewUnstartedServer(s)
	s.HTTPTestServer.Start()
	t.Cleanup(s.HTTPTestServer.Close)
	c := &Control{URL: s.HTTPTestServer.URL, AuthKey: o.AuthKey, Server: s}
	mux := http.NewServeMux()
	mux.HandleFunc("POST /complete", func(w http.ResponseWriter, r *http.Request) {
		b, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 16<<10))
		if err != nil {
			http.Error(w, "invalid auth URL body", http.StatusBadRequest)
			return
		}
		if !c.CompleteAuth(strings.TrimSpace(string(b))) {
			http.NotFound(w, r)
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	admin := httptest.NewServer(mux)
	c.AdminURL = admin.URL
	t.Cleanup(admin.Close)
	return c
}

func (c *Control) CompleteAuth(url string) bool { return c.Server.CompleteAuth(url) }

type Peer struct {
	Srv      *tsnet.Server
	Hostname string
	DNSName  string
	IP4      netip.Addr
	IPs      []netip.Addr
}

func StartPeer(t testing.TB, c *Control, hostname string) *Peer {
	t.Helper()
	s := &tsnet.Server{
		Dir: t.TempDir(), Store: new(mem.Store), Hostname: hostname,
		ControlURL: c.URL, AuthKey: c.AuthKey, Ephemeral: true,
		Logf: logger.Discard, UserLogf: logger.Discard,
	}
	if err := s.Start(); err != nil {
		if s.Sys() != nil {
			s.Close()
		}
		t.Fatalf("start peer: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if c.Server.RequireAuth {
		lc, err := s.LocalClient()
		if err != nil {
			t.Fatalf("peer local client: %v", err)
		}
		authCtx, authCancel := context.WithTimeout(ctx, 10*time.Second)
		defer authCancel()
		tick := time.NewTicker(20 * time.Millisecond)
		defer tick.Stop()
		for {
			st, err := lc.StatusWithoutPeers(authCtx)
			if err != nil {
				t.Fatalf("peer login status: %v", err)
			}
			if st.AuthURL != "" {
				if !c.CompleteAuth(st.AuthURL) {
					t.Fatal("peer auth URL not found")
				}
				break
			}
			select {
			case <-authCtx.Done():
				t.Fatal("peer auth URL timed out")
			case <-tick.C:
			}
		}
	}
	st, err := s.Up(ctx)
	if err != nil {
		t.Fatalf("peer Up: %v", err)
	}
	// testcontrol does not populate Online itself. Its Node/UpdateNode API
	// clones under its mutex, so publish the running fixture peer explicitly
	// without mutating any live server configuration.
	registered := c.Server.Node(st.Self.PublicKey)
	if registered == nil {
		t.Fatal("running peer is missing from testcontrol")
	}
	online := true
	registered.Online = &online
	c.Server.UpdateNode(registered)
	p := &Peer{Srv: s, Hostname: hostname, DNSName: st.Self.DNSName, IPs: st.TailscaleIPs}
	for _, ip := range p.IPs {
		if ip.Is4() {
			p.IP4 = ip
			break
		}
	}
	if !p.IP4.IsValid() {
		t.Fatal("peer has no IPv4 address")
	}
	return p
}

func (p *Peer) address(port uint16) string {
	return net.JoinHostPort(p.IP4.String(), strconv.Itoa(int(port)))
}

func (p *Peer) UDPEcho(t testing.TB, port uint16) {
	t.Helper()
	pc, err := p.Srv.ListenPacket("udp4", p.address(port))
	if err != nil {
		t.Fatalf("UDP echo listen: %v", err)
	}
	done := make(chan struct{})
	t.Cleanup(func() {
		pc.Close()
		<-done
	})
	go func() {
		defer close(done)
		buf := make([]byte, 65535)
		for {
			n, src, err := pc.ReadFrom(buf)
			if err != nil {
				return
			}
			if _, err := pc.WriteTo(buf[:n], src); err != nil {
				return
			}
		}
	}()
}

func closeWrite(c net.Conn) {
	if cw, ok := c.(interface{ CloseWrite() error }); ok {
		cw.CloseWrite()
	}
}

func (p *Peer) TCPEcho(t testing.TB, port uint16) {
	t.Helper()
	p.serveTCP(t, port, func(ctx context.Context, c net.Conn) {
		io.Copy(c, c)
		closeWrite(c)
	})
}

// serveTCP owns accepted connections and joins every handler on cleanup.
func (p *Peer) serveTCP(t testing.TB, port uint16, handle func(context.Context, net.Conn)) {
	t.Helper()
	ln, err := p.Srv.Listen("tcp4", p.address(port))
	if err != nil {
		t.Fatalf("TCP listen: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	var wg sync.WaitGroup
	wg.Add(1)
	t.Cleanup(func() {
		cancel()
		ln.Close()
		wg.Wait()
	})
	go func() {
		defer wg.Done()
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			wg.Add(1)
			go func() {
				defer wg.Done()
				defer c.Close()
				stop := context.AfterFunc(ctx, func() { c.Close() })
				defer stop()
				handle(ctx, c)
			}()
		}
	}()
}

func (p *Peer) ForwardTCP(t testing.TB, port uint16, target string) {
	t.Helper()
	validateTarget(t, target)
	p.serveTCP(t, port, func(ctx context.Context, c net.Conn) {
		upstream, err := (&net.Dialer{Timeout: 5 * time.Second}).DialContext(ctx, "tcp4", target)
		if err != nil {
			return
		}
		defer upstream.Close()
		stop := context.AfterFunc(ctx, func() { upstream.Close() })
		defer stop()
		done := make(chan struct{})
		go func() {
			defer close(done)
			io.Copy(upstream, c)
			closeWrite(upstream)
		}()
		io.Copy(c, upstream)
		closeWrite(c)
		<-done
	})
}

// Test forwards deliberately accept only numeric loopback targets: no DNS or
// accidental traffic to a shared/private network from the helper process.
func validateTarget(t testing.TB, target string) {
	t.Helper()
	a, err := netip.ParseAddrPort(target)
	if err != nil || !a.Addr().Is4() || !a.Addr().IsLoopback() || a.Port() == 0 {
		t.Fatalf("forward target must be IPv4 loopback with a nonzero port: %q", target)
	}
}

type udpFlow struct {
	conn *net.UDPConn
	src  net.Addr
	last time.Time
}

func (p *Peer) ForwardUDP(t testing.TB, port uint16, target string) {
	t.Helper()
	validateTarget(t, target)
	addr, err := net.ResolveUDPAddr("udp4", target)
	if err != nil {
		t.Fatalf("UDP forward target: %v", err)
	}
	pc, err := p.Srv.ListenPacket("udp4", p.address(port))
	if err != nil {
		t.Fatalf("UDP forward listen: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	var mu sync.Mutex
	flows := make(map[string]*udpFlow)
	var wg sync.WaitGroup
	wg.Add(2)
	t.Cleanup(func() {
		cancel()
		pc.Close()
		mu.Lock()
		for key, f := range flows {
			f.conn.Close()
			delete(flows, key)
		}
		mu.Unlock()
		wg.Wait()
	})
	go func() {
		defer wg.Done()
		ticker := time.NewTicker(time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case now := <-ticker.C:
				mu.Lock()
				for key, f := range flows {
					if now.Sub(f.last) >= 60*time.Second {
						f.conn.Close()
						delete(flows, key)
					}
				}
				mu.Unlock()
			}
		}
	}()
	go func() {
		defer wg.Done()
		buf := make([]byte, 65535)
		for {
			n, src, err := pc.ReadFrom(buf)
			if err != nil {
				return
			}
			key := src.String()
			mu.Lock()
			if ctx.Err() != nil {
				mu.Unlock()
				return
			}
			f := flows[key]
			if f == nil {
				conn, err := net.DialUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)}, addr)
				if err != nil {
					mu.Unlock()
					continue
				}
				f = &udpFlow{conn: conn, src: src}
				flows[key] = f
				wg.Add(1)
				go func(f *udpFlow) {
					defer wg.Done()
					defer f.conn.Close()
					defer func() {
						mu.Lock()
						if flows[key] == f {
							delete(flows, key)
						}
						mu.Unlock()
					}()
					buf := make([]byte, 65535)
					for {
						n, err := f.conn.Read(buf)
						if err != nil {
							return
						}
						mu.Lock()
						f.last = time.Now()
						mu.Unlock()
						if _, err := pc.WriteTo(buf[:n], f.src); err != nil {
							return
						}
					}
				}(f)
			}
			f.last = time.Now()
			mu.Unlock()
			f.conn.Write(buf[:n])
		}
	}()
}
