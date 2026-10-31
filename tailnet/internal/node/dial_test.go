//go:build unix

package node

import (
	"bytes"
	"crypto/rand"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"remotescreen/tailnet/internal/testnet"
	"tailscale.com/tailcfg"
)

func dialFixture(t *testing.T, idle int) (*Node, *testnet.Peer, int) {
	t.Helper()
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{})
	p := testnet.StartPeer(t, c, "rs-peer")
	p.UDPEcho(t, 7401)
	p.TCPEcho(t, 7402)
	baseline := runtime.NumGoroutine()
	n := New()
	cfg := Config{Dir: t.TempDir(), Hostname: "rs-client", ControlURL: c.URL, DisableNetns: true, UDPIdleSeconds: idle}
	if err := n.Start(cfg); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stopWithin(t, n, 20*time.Second) })
	awaitRunning(t, ctx, n)
	awaitStatus(t, ctx, n, 5*time.Second, func(s Status) bool { return len(s.Peers) == 1 })
	// TSMP, unlike disco, needs the WireGuard handshake, so UDP sent after it
	// is not lost to a five-second handshake retry.
	for {
		res, err := n.localClient().Ping(ctx, p.IP4, tailcfg.PingTSMP)
		if err == nil && res.Err == "" {
			break
		}
		if ctx.Err() != nil {
			t.Fatalf("peer ping: %v", err)
		}
		time.Sleep(20 * time.Millisecond)
	}
	return n, p, baseline
}

// stopWithin reports a Stop that does not return instead of hanging the test
// binary. A stuck Stop keeps the node's lifecycle lock, so the goroutine leaks.
func stopWithin(t *testing.T, n *Node, limit time.Duration) {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- n.Stop() }()
	select {
	case err := <-done:
		if err != nil {
			t.Error(err)
		}
	case <-time.After(limit):
		buf := make([]byte, 1<<20)
		t.Errorf("Stop still blocked after %v; goroutines:\n%s", limit, buf[:runtime.Stack(buf, true)])
	}
}

// Inspect the exported descriptor before FileConn makes its own nonblocking copy.
func callerConn(t *testing.T, fd, kind int) *net.UnixConn {
	t.Helper()
	f := os.NewFile(uintptr(fd), "tailnet-caller")
	defer f.Close()
	got, err := syscall.GetsockoptInt(fd, syscall.SOL_SOCKET, syscall.SO_TYPE)
	if err != nil || got != kind {
		t.Fatalf("SO_TYPE = %d, %v; want %d", got, err, kind)
	}
	addr, err := syscall.Getsockname(fd)
	if _, ok := addr.(*syscall.SockaddrUnix); !ok || err != nil {
		t.Fatalf("Getsockname = %T, %v", addr, err)
	}
	flags, _, errno := syscall.Syscall(syscall.SYS_FCNTL, uintptr(fd), syscall.F_GETFD, 0)
	if errno != 0 || flags&syscall.FD_CLOEXEC == 0 {
		t.Fatalf("fd not CLOEXEC: %x, %v", flags, errno)
	}
	flags, _, errno = syscall.Syscall(syscall.SYS_FCNTL, uintptr(fd), syscall.F_GETFL, 0)
	if errno != 0 || flags&syscall.O_NONBLOCK != 0 {
		t.Fatalf("fd not blocking: %x, %v", flags, errno)
	}
	conn, err := net.FileConn(f)
	if err != nil {
		t.Fatal(err)
	}
	u, ok := conn.(*net.UnixConn)
	if !ok {
		conn.Close()
		t.Fatalf("caller conn = %T", conn)
	}
	t.Cleanup(func() { u.Close() })
	if err := u.SetDeadline(time.Now().Add(10 * time.Second)); err != nil {
		t.Fatal(err)
	}
	return u
}

func awaitReclaimed(t *testing.T, n *Node, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for n.liveTracked() != 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if got := n.liveTracked(); got != 0 {
		t.Fatalf("%d relays still tracked", got)
	}
}

func TestDialUDPBoundaries(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	fd, err := n.DialUDP(p.IP4.String(), 7401, 0)
	if err != nil {
		t.Fatal(err)
	}
	c := callerConn(t, fd, syscall.SOCK_DGRAM)
	received := 0
	for _, size := range []int{1, 512, 1200, 1252} {
		for i := range 50 {
			payload := bytes.Repeat([]byte{byte(i + 1)}, size)
			if count, err := c.Write(payload); err != nil || count != size {
				t.Fatalf("write: %d, %v", count, err)
			}
			c.SetReadDeadline(time.Now().Add(time.Second))
			buf := make([]byte, 64<<10)
			count, err := c.Read(buf)
			if err != nil {
				if os.IsTimeout(err) {
					continue
				}
				t.Fatal(err)
			}
			if count != size || !bytes.Equal(buf[:count], payload) {
				t.Fatalf("packet size=%d sequence=%d: got %d corrupt bytes", size, i, count)
			}
			received++
		}
	}
	t.Logf("UDP recovery: %d/200 (%.1f%%)", received, float64(received)/2)
	if received < 190 {
		t.Fatal("UDP recovery below 95%")
	}
}

func TestUDPZeroLengthFromTailnetDropped(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	pc, err := p.Srv.ListenPacket("udp4", net.JoinHostPort(p.IP4.String(), "7403"))
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	t.Cleanup(func() { pc.Close() })
	go func() {
		buf := make([]byte, 10)
		size, src, err := pc.ReadFrom(buf)
		if err == nil && string(buf[:size]) != "go" {
			err = fmt.Errorf("unexpected trigger")
		}
		if err == nil {
			_, err = pc.WriteTo(nil, src)
		}
		if err == nil {
			_, err = pc.WriteTo([]byte("after"), src)
		}
		done <- err
	}()
	fd, err := n.DialUDP(p.IP4.String(), 7403, 0)
	if err != nil {
		t.Fatal(err)
	}
	c := callerConn(t, fd, syscall.SOCK_DGRAM)
	if _, err := c.Write([]byte("go")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 10)
	size, err := c.Read(buf)
	if err != nil || size != 5 || string(buf[:size]) != "after" {
		t.Fatalf("first read: %d, %q, %v", size, buf[:size], err)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestStopSignalsFds(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	var udp, tcp []*net.UnixConn
	for range 2 {
		fd, err := n.DialUDP(p.IP4.String(), 7401, 0)
		if err != nil {
			t.Fatal(err)
		}
		udp = append(udp, callerConn(t, fd, syscall.SOCK_DGRAM))
		fd, err = n.DialTCP(p.IP4.String(), 7402, 0)
		if err != nil {
			t.Fatal(err)
		}
		tcp = append(tcp, callerConn(t, fd, syscall.SOCK_STREAM))
	}
	if got := n.liveTracked(); got != 4 {
		t.Fatalf("tracked=%d", got)
	}
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for _, c := range udp {
		c.SetReadDeadline(deadline)
		size, err := c.Read(make([]byte, 1))
		if size != 0 || err != nil {
			t.Fatalf("UDP close signal: %d, %v", size, err)
		}
	}
	for _, c := range tcp {
		c.SetReadDeadline(deadline)
		size, err := c.Read(make([]byte, 1))
		if size != 0 || err != io.EOF {
			t.Fatalf("TCP close: %d, %v", size, err)
		}
	}
	awaitReclaimed(t, n, time.Second)
}

// tsnet holds Server.mu for all of Close while WireGuard drains its receive
// goroutines, and a packet for an unbound UDP port takes Server.mu there.
func TestStopWhilePeerSendsUDP(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	ctx := testContext(t)
	var self string
	for _, ip := range readStatus(t, ctx, n).Self.IPs {
		if net.ParseIP(ip).To4() != nil {
			self = ip
		}
	}
	conn, err := p.Srv.Dial(ctx, "udp", net.JoinHostPort(self, "9"))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	quit, sent := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(sent)
		tick := time.NewTicker(time.Millisecond)
		defer tick.Stop()
		for {
			select {
			case <-quit:
				return
			case <-tick.C:
				conn.Write([]byte("x"))
			}
		}
	}()
	defer func() { close(quit); <-sent }()
	time.Sleep(100 * time.Millisecond)
	stopWithin(t, n, 15*time.Second)
}

func TestUDPCallerCloseReclaimed(t *testing.T) {
	n, p, _ := dialFixture(t, 1)
	fd, err := n.DialUDP(p.IP4.String(), 7401, 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := syscall.Close(fd); err != nil {
		t.Fatal(err)
	}
	awaitReclaimed(t, n, 3*time.Second)
}

func TestDialTCPByIPAndNames(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	for _, host := range []string{p.IP4.String(), strings.TrimSuffix(p.DNSName, "."), "rs-peer"} {
		t.Run(host, func(t *testing.T) {
			fd, err := n.DialTCP(host, 7402, 0)
			if err != nil {
				t.Fatal(err)
			}
			c := callerConn(t, fd, syscall.SOCK_STREAM)
			payload := make([]byte, 1<<20)
			if _, err := rand.Read(payload); err != nil {
				t.Fatal(err)
			}
			writeDone := make(chan error, 1)
			go func() {
				_, err := io.Copy(c, bytes.NewReader(payload))
				if err == nil {
					err = c.CloseWrite()
				} // shutdown(SHUT_WR)
				writeDone <- err
			}()
			got, err := io.ReadAll(c)
			writeErr := <-writeDone
			if err != nil || writeErr != nil || !bytes.Equal(got, payload) {
				t.Fatalf("TCP half-close: bytes=%d read=%v write=%v", len(got), err, writeErr)
			}
			awaitReclaimed(t, n, 2*time.Second)
		})
	}
}

func TestDialErrors(t *testing.T) {
	for _, dial := range []func(string, uint16, time.Duration) (int, error){New().DialUDP, New().DialTCP} {
		if _, err := dial("100.64.0.1", 7401, 0); CodeOf(err) != CodeState {
			t.Fatalf("before start: %v", err)
		}
	}
	t.Run("not running", func(t *testing.T) {
		c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
		n, _ := startNode(t, c, "")
		for _, dial := range []func(string, uint16, time.Duration) (int, error){n.DialUDP, n.DialTCP} {
			start := time.Now()
			_, err := dial("100.64.0.1", 7401, time.Second)
			if CodeOf(err) != CodeNotRunning || time.Since(start) >= 200*time.Millisecond {
				t.Fatalf("gate: code=%d duration=%v", CodeOf(err), time.Since(start))
			}
		}
	})
	n, p, _ := dialFixture(t, 0)
	for _, tc := range []struct {
		host string
		port uint16
	}{{"abc", 7401}, {"100.64.0.1", 0}, {"fe80::1%lo", 7401}} {
		if _, err := n.DialUDP(tc.host, tc.port, 0); CodeOf(err) != CodeInvalid {
			t.Fatalf("invalid UDP: %v", err)
		}
	}
	for _, tc := range []struct {
		host string
		port uint16
	}{{"", 7401}, {"rs peer", 7401}, {"rs\tpeer", 7401}, {"rs\u2003peer", 7401}, {"rs-peer", 0}} {
		if _, err := n.DialTCP(tc.host, tc.port, 0); CodeOf(err) != CodeInvalid {
			t.Fatalf("invalid TCP: %v", err)
		}
	}
	if _, err := n.DialTCP(p.IP4.String(), 7499, time.Second); CodeOf(err) != CodeRefused {
		t.Fatalf("refused: %v (code %d)", err, CodeOf(err))
	}
	for _, host := range []string{"100.64.99.99", "nosuchhost"} {
		_, err := n.DialTCP(host, 1, 500*time.Millisecond)
		code := CodeOf(err)
		t.Logf("dial %s: code=%d", host, code)
		if code != CodeTimeout && code != CodeUnreachable {
			t.Fatalf("unreachable: %v (code %d)", err, code)
		}
	}
}

func TestConcurrentDials(t *testing.T) {
	n, p, baseline := dialFixture(t, 0)
	var wg sync.WaitGroup
	for i := range 32 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			fd, err := n.DialUDP(p.IP4.String(), 7401, 0)
			if err != nil {
				t.Error(err)
				return
			}
			c := callerConn(t, fd, syscall.SOCK_DGRAM)
			defer c.Close()
			payload := []byte{byte(i + 1)}
			if _, err := c.Write(payload); err != nil {
				t.Error(err)
				return
			}
			buf := make([]byte, 32)
			size, err := c.Read(buf)
			if err != nil || !bytes.Equal(buf[:size], payload) {
				t.Errorf("concurrent echo: %d, %v", size, err)
			}
		}()
	}
	wg.Wait()
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for runtime.NumGoroutine() > baseline+10 && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if got := runtime.NumGoroutine(); got > baseline+10 {
		t.Fatalf("goroutines after Stop: %d, baseline=%d", got, baseline)
	}
	awaitReclaimed(t, n, time.Second)
}

func TestInteractiveLoginDoesNotLogAuthURL(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	path := filepath.Join(t.TempDir(), "tailnet.log")
	n := New()
	if err := n.Start(Config{Dir: t.TempDir(), Hostname: "rs-client", ControlURL: c.URL, DisableNetns: true, LogFile: path, Verbose: 1}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { n.Stop() })
	url := awaitAuth(t, ctx, n)
	completeAuth(t, ctx, c, url)
	awaitRunning(t, ctx, n)
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(b, []byte(url)) || bytes.Contains(b, []byte("/auth/")) {
		t.Fatal("login log exposes auth URL or auth path")
	}
}

func TestConcurrentDialAndStop(t *testing.T) {
	n, p, _ := dialFixture(t, 0)
	var wg sync.WaitGroup
	start := make(chan struct{})
	for range 32 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			fd, err := n.DialUDP(p.IP4.String(), 7401, time.Second)
			if err == nil {
				syscall.Close(fd)
			}
		}()
	}
	close(start)
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	wg.Wait()
	awaitReclaimed(t, n, 2*time.Second)
}

func TestUDPFullCallerQueueDoesNotBlockClose(t *testing.T) {
	fd, unix, err := relaySocketpair(syscall.SOCK_DGRAM)
	if err != nil {
		t.Fatal(err)
	}
	defer syscall.Close(fd)
	defer unix.Close()
	left, right := net.Pipe()
	defer right.Close()
	r := &udpRelay{relay: newRelay(unix, left), started: time.Now()}
	defer r.Close()
	raw, err := unix.SyscallConn()
	if err != nil {
		t.Fatal(err)
	}
	var fillErr error
	if err := raw.Control(func(fd uintptr) {
		buf := make([]byte, 4096)
		for range 10000 {
			_, fillErr = syscall.Write(int(fd), buf)
			if fillErr != nil {
				return
			}
		}
	}); err != nil {
		t.Fatal(err)
	}
	if fillErr != syscall.EAGAIN && fillErr != syscall.ENOBUFS {
		t.Fatalf("queue not full: %v", fillErr)
	}
	done := make(chan struct{})
	go func() { r.Close(); close(done) }()
	select {
	case <-done:
	case <-time.After(time.Second):
		unix.Close()
		<-done
		t.Fatal("close blocked on the caller receive queue")
	}
}

func TestTailnetLogRedactsAuthPaths(t *testing.T) {
	path := filepath.Join(t.TempDir(), "tailnet.log")
	log, err := openLog(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, message := range []string{
		"control: AuthURL is https://hs.example.com/a/private-login",
		"Received auth URL: https://hs.example...",
		"To start this tsnet server, restart with TS_AUTHKEY set, or go to: https://hs.example.com/a/private-login",
		"control response: newURL=https://hs.example.com/a/private-login, err=nil",
		"netmap: control says to open URL https://hs.example.com/a/private-login; opening...",
		"a multiline message\nhttps://hs.example.com/auth/private-login",
	} {
		log.logf("%s", message)
	}
	log.logf("safe record %d", 1)
	if err := log.Close(); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(b, []byte("https://")) || bytes.Contains(b, []byte("/auth/")) || bytes.Count(b, []byte("authUrl issued")) != 6 || !bytes.Contains(b, []byte("safe record 1")) {
		t.Fatal("logger must redact auth messages and preserve ordinary records")
	}
}
