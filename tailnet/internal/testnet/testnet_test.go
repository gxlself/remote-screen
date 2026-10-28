package testnet

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"tailscale.com/ipn"
	"tailscale.com/ipn/ipnstate"
	stateStore "tailscale.com/ipn/store"
	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
	"tailscale.com/types/logger"
)

func newStateStore(t testing.TB, dir string) ipn.StateStore {
	t.Helper()
	store, err := stateStore.NewFileStore(logger.Discard, filepath.Join(dir, "state"))
	if err != nil {
		t.Fatalf("new state store: %v", err)
	}
	return store
}

func startAuthPeer(t testing.TB, c *Control, dir string, store ipn.StateStore, authKey string) *tsnet.Server {
	t.Helper()
	s := &tsnet.Server{
		Dir: dir, Store: store, Hostname: "rs-auth-peer",
		ControlURL: c.URL, AuthKey: authKey,
		Logf: logger.Discard, UserLogf: logger.Discard,
	}
	if err := s.Start(); err != nil {
		t.Fatalf("start auth peer: %v", err)
	}
	return s
}

func waitNeedsLogin(t testing.TB, s *tsnet.Server) *ipnstate.Status {
	t.Helper()
	lc, err := s.LocalClient()
	if err != nil {
		t.Fatalf("peer local client: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	tick := time.NewTicker(20 * time.Millisecond)
	defer tick.Stop()
	for {
		st, err := lc.StatusWithoutPeers(ctx)
		if err != nil {
			t.Fatalf("peer login status: %v", err)
		}
		if st.BackendState == "NeedsLogin" {
			return st
		}
		select {
		case <-ctx.Done():
			t.Fatalf("peer did not reach NeedsLogin: %v", ctx.Err())
		case <-tick.C:
		}
	}
}

func statusIPv4(t testing.TB, st *ipnstate.Status) netip.Addr {
	t.Helper()
	for _, ip := range st.TailscaleIPs {
		if ip.Is4() {
			return ip
		}
	}
	t.Fatalf("status has no IPv4 address: %+v", st)
	return netip.Addr{}
}

func TestAuthKeyNewNodeWithoutKeyNeedsLogin(t *testing.T) {
	c := StartControl(t, ControlOptions{AuthKey: "tskey-rs-test"})
	dir := t.TempDir()
	s := startAuthPeer(t, c, dir, newStateStore(t, dir), "")
	defer s.Close()
	waitNeedsLogin(t, s)
	if got := c.Server.NumNodes(); got != 0 {
		t.Fatalf("control registered unauthenticated node: NumNodes() = %d", got)
	}
}

func TestAuthKeyNewNodeWithWrongKeyNeedsLogin(t *testing.T) {
	c := StartControl(t, ControlOptions{AuthKey: "tskey-rs-test"})
	dir := t.TempDir()
	s := startAuthPeer(t, c, dir, newStateStore(t, dir), "tskey-rs-wrong")
	defer s.Close()
	waitNeedsLogin(t, s)
	if got := c.Server.NumNodes(); got != 0 {
		t.Fatalf("control registered wrong-key node: NumNodes() = %d", got)
	}
}

func TestAuthKeyRestartWithoutKeyKeepsIdentity(t *testing.T) {
	c := StartControl(t, ControlOptions{AuthKey: "tskey-rs-test"})
	dir := t.TempDir()
	store := newStateStore(t, dir)
	first := startAuthPeer(t, c, dir, store, c.AuthKey)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	firstStatus, err := first.Up(ctx)
	cancel()
	if err != nil {
		t.Fatalf("first auth-key Up: %v", err)
	}
	if firstStatus.Self == nil {
		t.Fatal("first auth-key Up returned no self status")
	}
	firstIP := statusIPv4(t, firstStatus)
	firstKey := firstStatus.Self.PublicKey
	if got := c.Server.NumNodes(); got != 1 {
		t.Fatalf("first registration NumNodes() = %d, want 1", got)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("close first auth-key peer: %v", err)
	}

	second := startAuthPeer(t, c, dir, store, "")
	defer second.Close()
	ctx, cancel = context.WithTimeout(context.Background(), 30*time.Second)
	secondStatus, err := second.Up(ctx)
	cancel()
	if err != nil {
		t.Fatalf("restart without auth key: %v", err)
	}
	if secondStatus.Self == nil {
		t.Fatal("restart without auth key returned no self status")
	}
	if got := statusIPv4(t, secondStatus); got != firstIP {
		t.Fatalf("restart changed IPv4: %v -> %v", firstIP, got)
	}
	if secondStatus.Self.PublicKey != firstKey {
		t.Fatalf("restart changed node key: %v -> %v", firstKey, secondStatus.Self.PublicKey)
	}
	if got := c.Server.NumNodes(); got != 1 {
		t.Fatalf("restart changed NumNodes() = %d, want 1", got)
	}
}

func TestAuthKeyFreshNodeWithoutKeyAfterRegistrationNeedsLogin(t *testing.T) {
	c := StartControl(t, ControlOptions{AuthKey: "tskey-rs-test"})
	dir := t.TempDir()
	first := startAuthPeer(t, c, dir, newStateStore(t, dir), c.AuthKey)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	_, err := first.Up(ctx)
	cancel()
	if err != nil {
		t.Fatalf("first auth-key Up: %v", err)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("close first auth-key peer: %v", err)
	}
	if got := c.Server.NumNodes(); got != 1 {
		t.Fatalf("first registration NumNodes() = %d, want 1", got)
	}

	freshDir := t.TempDir()
	fresh := startAuthPeer(t, c, freshDir, newStateStore(t, freshDir), "")
	defer fresh.Close()
	waitNeedsLogin(t, fresh)
	if got := c.Server.NumNodes(); got != 1 {
		t.Fatalf("fresh unauthenticated node changed NumNodes() = %d, want 1", got)
	}
}

func TestStartPeerInteractive(t *testing.T) {
	c := StartControl(t, ControlOptions{RequireAuth: true})
	p := StartPeer(t, c, "rs-peer")
	if p.DNSName != "rs-peer.rs.test." || !p.IP4.IsValid() || len(p.IPs) < 1 {
		t.Fatalf("peer = %+v", p)
	}
	lc, err := p.Srv.LocalClient()
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := lc.Status(ctx)
	if err != nil || st.BackendState != "Running" {
		t.Fatalf("status = %+v, %v", st, err)
	}
	if c.CompleteAuth("/auth/not-found") {
		t.Fatal("unknown auth URL succeeded")
	}
	request, err := http.NewRequestWithContext(ctx, "POST", c.AdminURL+"/complete", strings.NewReader("/auth/not-found"))
	if err != nil {
		t.Fatal(err)
	}
	res, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	if res.StatusCode != http.StatusNotFound {
		t.Fatalf("admin status = %d", res.StatusCode)
	}
}

func TestEcho(t *testing.T) {
	c := StartControl(t, ControlOptions{AuthKey: "tskey-rs-test"})
	p := StartPeer(t, c, "rs-peer")
	p.UDPEcho(t, 7401)
	p.TCPEcho(t, 7402)
	client := StartPeer(t, c, "rs-client")
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	waitPeer(t, ctx, client, p)
	udp, err := client.Srv.Dial(ctx, "udp4", p.address(7401))
	if err != nil {
		t.Fatal(err)
	}
	defer udp.Close()
	if err := udp.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatal(err)
	}
	for _, size := range []int{0, 1, 1200} {
		payload := bytes.Repeat([]byte{0x5a}, size)
		if n, err := udp.Write(payload); err != nil || n != size {
			t.Fatalf("UDP write %d: %d, %v", size, n, err)
		}
		buf := make([]byte, 1500)
		n, err := udp.Read(buf)
		if err != nil || n != size || !bytes.Equal(buf[:n], payload) {
			t.Fatalf("UDP read %d: %d, %v", size, n, err)
		}
	}
	tcp, err := client.Srv.Dial(ctx, "tcp4", p.address(7402))
	if err != nil {
		t.Fatal(err)
	}
	defer tcp.Close()
	checkTCPRoundtrip(t, tcp)
}

func waitPeer(t *testing.T, ctx context.Context, client, peer *Peer) {
	t.Helper()
	lc, err := client.Srv.LocalClient()
	if err != nil {
		t.Fatal(err)
	}
	tick := time.NewTicker(20 * time.Millisecond)
	defer tick.Stop()
	// TSMP goes through WireGuard, so a reply means the handshake is done. A
	// disco reply does not: if the first handshake initiation is lost, UDP
	// stays dark until WireGuard retries it five seconds later.
	for {
		res, err := lc.Ping(ctx, peer.IP4, tailcfg.PingTSMP)
		if err == nil && res.Err == "" {
			return
		}
		select {
		case <-ctx.Done():
			t.Fatalf("peer ping: %v (last error %v)", ctx.Err(), err)
		case <-tick.C:
		}
	}
}

func checkTCPRoundtrip(t *testing.T, c net.Conn) {
	t.Helper()
	if err := c.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatal(err)
	}
	payload := bytes.Repeat([]byte{0x5a}, 64<<10)
	if len(payload) != 64<<10 {
		t.Fatalf("test payload has %d bytes", len(payload))
	}
	writeDone := make(chan error, 1)
	go func() {
		_, err := io.Copy(c, bytes.NewReader(payload))
		if err == nil {
			cw, ok := c.(interface{ CloseWrite() error })
			if !ok {
				err = fmt.Errorf("%T has no CloseWrite", c)
			} else {
				err = cw.CloseWrite()
			}
		}
		writeDone <- err
	}()
	got, err := io.ReadAll(c)
	writeErr := <-writeDone
	if err != nil || writeErr != nil || !bytes.Equal(got, payload) {
		t.Fatalf("TCP roundtrip: len=%d read=%v write=%v", len(got), err, writeErr)
	}
}

func TestForward(t *testing.T) {
	udp, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	udpDone := make(chan struct{})
	go func() {
		defer close(udpDone)
		buf := make([]byte, 65535)
		for {
			n, src, err := udp.ReadFromUDP(buf)
			if err != nil {
				return
			}
			udp.WriteToUDP(buf[:n], src)
		}
	}()
	t.Cleanup(func() { udp.Close(); <-udpDone })
	tcp, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			conn, err := tcp.Accept()
			if err != nil {
				return
			}
			wg.Add(1)
			go func() {
				defer wg.Done()
				defer conn.Close()
				conn.SetDeadline(time.Now().Add(10 * time.Second))
				// Wait for the forwarded half-close before responding.
				b, err := io.ReadAll(conn)
				if err == nil {
					conn.Write(b)
					closeWrite(conn)
				}
			}()
		}
	}()
	t.Cleanup(func() { tcp.Close(); wg.Wait() })
	c := StartControl(t, ControlOptions{})
	p := StartPeer(t, c, "rs-peer")
	p.ForwardUDP(t, 7400, udp.LocalAddr().String())
	p.ForwardTCP(t, 7400, tcp.Addr().String())
	client := StartPeer(t, c, "rs-client")
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	waitPeer(t, ctx, client, p)
	// Distinct sources must have independent return paths while all stay open.
	for range 3 {
		conn, err := client.Srv.Dial(ctx, "udp4", p.address(7400))
		if err != nil {
			t.Fatal(err)
		}
		defer conn.Close()
		conn.SetDeadline(time.Now().Add(5 * time.Second))
		for _, size := range []int{0, 1, 1200} {
			payload := bytes.Repeat([]byte{byte(size)}, size)
			if _, err := conn.Write(payload); err != nil {
				t.Fatal(err)
			}
			buf := make([]byte, 1500)
			n, err := conn.Read(buf)
			if err != nil || !bytes.Equal(buf[:n], payload) {
				t.Fatalf("forward UDP length=%d, %v", n, err)
			}
		}
	}
	conn, err := client.Srv.Dial(ctx, "tcp4", p.address(7400))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	checkTCPRoundtrip(t, conn)
}

func TestForwardFlags(t *testing.T) {
	var f forwardFlags
	for _, s := range []string{"7400=127.0.0.1:5555,7401=127.0.0.1:5556", "7402=127.0.0.1:5557"} {
		if err := f.Set(s); err != nil {
			t.Fatal(err)
		}
	}
	if len(f) != 3 {
		t.Fatalf("forwards = %v", f)
	}
	for _, s := range []string{"0=127.0.0.1:5555", "65536=127.0.0.1:5555", "7400", "7400="} {
		if err := f.Set(s); err == nil {
			t.Errorf("accepted %q", s)
		}
	}
}
