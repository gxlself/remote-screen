package node

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/netip"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"remotescreen/tailnet/internal/testnet"

	"tailscale.com/envknob"
	"tailscale.com/tailcfg"
)

type receivedEvent struct {
	Type    string  `json:"type"`
	State   string  `json:"state"`
	AuthURL *string `json:"authUrl"`
	URL     string  `json:"url"`
	Message string  `json:"message"`
}

func testContext(t *testing.T) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), 55*time.Second)
	t.Cleanup(cancel)
	return ctx
}

func startNode(t *testing.T, c *testnet.Control, authKey string) (*Node, Config) {
	t.Helper()
	cfg := Config{Dir: filepath.Join(t.TempDir(), "state"), Hostname: "rs-app-a1", ControlURL: c.URL, AuthKey: authKey, DisableNetns: true}
	n := New()
	if err := n.Start(cfg); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := n.Stop(); err != nil {
			t.Errorf("Stop: %v", err)
		}
	})
	if st, err := os.Stat(cfg.Dir); err != nil || st.Mode().Perm() != 0o700 {
		t.Fatalf("state directory permissions: %v, %v", st, err)
	}
	return n, cfg
}

func nextEvent(t *testing.T, n *Node, timeout time.Duration) receivedEvent {
	t.Helper()
	b, _, err := n.NextEvent(timeout, 64<<10)
	if err != nil {
		t.Fatalf("NextEvent: %v", err)
	}
	var ev receivedEvent
	if len(b) != 0 {
		if err := json.Unmarshal(b, &ev); err != nil {
			t.Fatal(err)
		}
	}
	return ev
}

func awaitEvent(t *testing.T, ctx context.Context, n *Node, timeout time.Duration, match func(receivedEvent) bool) receivedEvent {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) && ctx.Err() == nil {
		ev := nextEvent(t, n, 100*time.Millisecond)
		if match(ev) {
			return ev
		}
	}
	t.Fatalf("event timed out after %v (context: %v)", timeout, ctx.Err())
	return receivedEvent{}
}

func awaitRunning(t *testing.T, ctx context.Context, n *Node) {
	t.Helper()
	awaitEvent(t, ctx, n, 30*time.Second, func(ev receivedEvent) bool {
		if ev.Type == "error" {
			t.Fatalf("node error: %s", ev.Message)
		}
		return ev.Type == "state" && ev.State == "Running"
	})
	if _, _, err := n.running(); err != nil {
		t.Fatalf("running gate after Running event: %v", err)
	}
}

func readStatus(t *testing.T, ctx context.Context, n *Node) Status {
	t.Helper()
	b, err := n.StatusJSON(ctx)
	if err != nil {
		t.Fatal(err)
	}
	var s Status
	if err := json.Unmarshal(b, &s); err != nil {
		t.Fatal(err)
	}
	return s
}

func awaitStatus(t *testing.T, ctx context.Context, n *Node, timeout time.Duration, match func(Status) bool) Status {
	t.Helper()
	deadline := time.Now().Add(timeout)
	var last Status
	for time.Now().Before(deadline) && ctx.Err() == nil {
		last = readStatus(t, ctx, n)
		if match(last) {
			return last
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("status timed out: %+v", last)
	return last
}

func awaitAuth(t *testing.T, ctx context.Context, n *Node) string {
	t.Helper()
	var url string
	var stateURL string
	awaitEvent(t, ctx, n, 15*time.Second, func(ev receivedEvent) bool {
		if ev.Type == "error" {
			t.Fatalf("node error: %s", ev.Message)
		}
		if ev.Type == "authUrl" {
			url = ev.URL
			if !strings.Contains(url, "/auth/") {
				t.Fatalf("invalid auth URL: %q", url)
			}
		}
		if ev.Type == "state" && ev.State == "NeedsLogin" && ev.AuthURL != nil {
			stateURL = *ev.AuthURL
		}
		return url != "" && stateURL == url
	})
	return url
}

func completeAuth(t *testing.T, ctx context.Context, c *testnet.Control, url string) {
	t.Helper()
	req, err := http.NewRequestWithContext(ctx, "POST", c.AdminURL+"/complete", strings.NewReader(url))
	if err != nil {
		t.Fatal(err)
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	if res.StatusCode != http.StatusOK {
		t.Fatalf("complete auth: HTTP %d", res.StatusCode)
	}
}

func TestPreauthLogin(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{AuthKey: "tskey-rs-test"})
	testnet.StartPeer(t, c, "rs-peer")
	n, cfg := startNode(t, c, c.AuthKey)
	if err := n.Start(cfg); CodeOf(err) != CodeState {
		t.Fatalf("duplicate Start: %v", err)
	}
	awaitRunning(t, ctx, n)
	s := awaitStatus(t, ctx, n, 5*time.Second, func(s Status) bool {
		return s.Self != nil && s.Self.DERPRegion == "test" && len(s.Peers) == 1 && s.Peers[0].Online
	})
	if s.Peers[0].Hostname != "rs-peer" || s.MagicDNSSuffix != "rs.test" {
		t.Fatalf("status = %+v", s)
	}
	prefix := netip.MustParsePrefix("100.64.0.0/10")
	found := false
	for _, ip := range s.Self.IPs {
		a, err := netip.ParseAddr(ip)
		if err == nil && prefix.Contains(a) {
			found = true
		}
	}
	if !found {
		t.Fatalf("self has no tailnet IPv4: %v", s.Self.IPs)
	}
}

func TestWrongAuthKey(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{AuthKey: "tskey-rs-test"})
	n, _ := startNode(t, c, "tskey-rs-wrong")
	deadline := time.Now().Add(10 * time.Second)
	sawError := false
	for time.Now().Before(deadline) {
		ev := nextEvent(t, n, 100*time.Millisecond)
		if ev.Type == "state" && ev.State == "Running" {
			t.Fatal("wrong auth key reached Running")
		}
		if ev.Type == "error" {
			sawError = true
		}
	}
	s := readStatus(t, ctx, n)
	if s.State == "Running" || (!sawError && s.State != "NeedsLogin") {
		t.Fatalf("wrong-key status = %+v", s)
	}
}

func TestInteractiveLogin(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	testnet.StartPeer(t, c, "rs-peer")
	n, _ := startNode(t, c, "")
	url := awaitAuth(t, ctx, n)
	s := readStatus(t, ctx, n)
	if s.AuthURL == nil || *s.AuthURL != url {
		t.Fatalf("status auth URL differs: %+v", s)
	}
	completeAuth(t, ctx, c, url)
	awaitRunning(t, ctx, n)
}

func TestLoginRefreshesURL(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	oldURL := awaitAuth(t, ctx, n)
	if err := n.Login(ctx); err != nil {
		t.Fatal(err)
	}
	url := awaitAuth(t, ctx, n)
	if url == oldURL {
		t.Fatal("Login reused an in-progress auth URL")
	}
	completeAuth(t, ctx, c, url)
	awaitRunning(t, ctx, n)
}

func TestRestartKeepsIdentity(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, cfg := startNode(t, c, "")
	completeAuth(t, ctx, c, awaitAuth(t, ctx, n))
	awaitRunning(t, ctx, n)
	before := readStatus(t, ctx, n).Self.IPs
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	if s := readStatus(t, ctx, n); s.State != "Stopped" || s.Self != nil {
		t.Fatalf("stopped status = %+v", s)
	}
	if err := n.Start(cfg); err != nil {
		t.Fatal(err)
	}
	awaitEvent(t, ctx, n, 30*time.Second, func(ev receivedEvent) bool {
		if ev.Type == "authUrl" {
			t.Fatal("restart requested interactive login")
		}
		return ev.Type == "state" && ev.State == "Running"
	})
	after := readStatus(t, ctx, n).Self.IPs
	if !reflect.DeepEqual(before, after) {
		t.Fatalf("identity changed: %v -> %v", before, after)
	}
}

func TestLogoutThenLogin(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	oldURL := awaitAuth(t, ctx, n)
	completeAuth(t, ctx, c, oldURL)
	awaitRunning(t, ctx, n)
	if err := n.Logout(ctx); err != nil {
		t.Fatal(err)
	}
	prefs, err := n.localClient().GetPrefs(ctx)
	if err != nil || prefs.ControlURL != c.URL || prefs.Hostname != "rs-app-a1" {
		t.Fatalf("logout lost configured control/hostname: %v", err)
	}
	awaitEvent(t, ctx, n, 15*time.Second, func(ev receivedEvent) bool { return ev.Type == "state" && ev.State == "NeedsLogin" })
	if err := n.Login(ctx); err != nil {
		t.Fatal(err)
	}
	url := awaitAuth(t, ctx, n)
	if url == oldURL {
		t.Fatal("login reused the old auth URL")
	}
	completeAuth(t, ctx, c, url)
	awaitRunning(t, ctx, n)
}

func TestRunningGate(t *testing.T) {
	if _, _, err := New().running(); CodeOf(err) != CodeState {
		t.Fatalf("before start: %v", err)
	}
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	start := time.Now()
	_, _, err := n.running()
	if CodeOf(err) != CodeNotRunning || time.Since(start) >= 200*time.Millisecond {
		t.Fatalf("running gate: %v, duration %v", err, time.Since(start))
	}
}

func TestPeerDirect(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{AuthKey: "tskey-rs-test"})
	p := testnet.StartPeer(t, c, "rs-peer")
	n, _ := startNode(t, c, c.AuthKey)
	awaitRunning(t, ctx, n)
	awaitStatus(t, ctx, n, 5*time.Second, func(s Status) bool { return len(s.Peers) == 1 })
	pingCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	for {
		res, err := n.localClient().Ping(pingCtx, p.IP4, tailcfg.PingDisco)
		if err != nil {
			t.Fatal(err)
		}
		if res.Err == "" && res.Endpoint != "" {
			break
		}
		if pingCtx.Err() != nil {
			t.Fatal("direct ping timed out")
		}
		time.Sleep(50 * time.Millisecond)
	}
	awaitStatus(t, ctx, n, 20*time.Second, func(s Status) bool {
		for _, peer := range s.Peers {
			if peer.Hostname == p.Hostname && peer.Direct && peer.CurAddr != "" {
				return true
			}
		}
		return false
	})
}

type countCloser struct{ calls atomic.Int32 }

func (c *countCloser) Close() error { c.calls.Add(1); return nil }

func TestStopClosesTracked(t *testing.T) {
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	closers := []*countCloser{{}, {}, {}}
	for _, c := range closers {
		n.track(c)
	}
	untracked := &countCloser{}
	untrack := n.track(untracked)
	untrack()
	untrack()
	if n.liveTracked() != 3 {
		t.Fatalf("tracked = %d", n.liveTracked())
	}
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	if n.liveTracked() != 0 {
		t.Fatal("resources remain tracked")
	}
	for _, c := range closers {
		if c.calls.Load() != 1 {
			t.Errorf("Close called %d times", c.calls.Load())
		}
	}
	if untracked.calls.Load() != 0 {
		t.Fatal("untracked resource was closed")
	}
	late := &countCloser{}
	n.track(late)()
	if late.calls.Load() != 1 {
		t.Fatal("late track did not close immediately")
	}
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
	for {
		ev := nextEvent(t, n, 0)
		if ev.Type == "stopped" {
			break
		}
		if ev.Type == "" {
			t.Fatal("no stopped event")
		}
	}
	if _, _, err := n.NextEvent(time.Second, 4096); CodeOf(err) != CodeClosed {
		t.Fatalf("after stopped: %v", err)
	}
}

func TestNoLogUpload(t *testing.T) {
	c := testnet.StartControl(t, testnet.ControlOptions{})
	startNode(t, c, "")
	if !envknob.NoLogsNoSupport() {
		t.Fatal("log uploads not disabled")
	}
}

func TestRebind(t *testing.T) {
	ctx := testContext(t)
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	if err := n.Rebind(ctx); CodeOf(err) != CodeNotRunning {
		t.Fatalf("rebind before login = %v", err)
	}
	completeAuth(t, ctx, c, awaitAuth(t, ctx, n))
	awaitRunning(t, ctx, n)
	if err := n.Rebind(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestConcurrentStopAndTrack(t *testing.T) {
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	n, _ := startNode(t, c, "")
	var wg sync.WaitGroup
	closers := make([]countCloser, 32)
	for i := range closers {
		wg.Add(1)
		go func() { defer wg.Done(); n.track(&closers[i]) }()
	}
	wg.Add(1)
	go func() {
		defer wg.Done()
		if err := n.Stop(); err != nil {
			t.Error(err)
		}
	}()
	wg.Wait()
	for i := range closers {
		if closers[i].calls.Load() != 1 {
			t.Errorf("closer %d calls = %d", i, closers[i].calls.Load())
		}
	}
}

func TestLogRotation(t *testing.T) {
	path := filepath.Join(t.TempDir(), "tailnet.log")
	old := bytes.Repeat([]byte("x"), maxLogSize)
	if err := os.WriteFile(path, old, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path+".1", []byte("older"), 0o600); err != nil {
		t.Fatal(err)
	}
	log, err := openLog(path)
	if err != nil {
		t.Fatal(err)
	}
	log.logf("new record %d", 1)
	if err := log.Close(); err != nil {
		t.Fatal(err)
	}
	backup, err := os.ReadFile(path + ".1")
	if err != nil || !bytes.Equal(backup, old) {
		t.Fatalf("backup differs, %v", err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	data, err := io.ReadAll(f)
	if err != nil || !bytes.Contains(data, []byte("new record 1")) {
		t.Fatalf("new log = %q, %v", data, err)
	}
	st, err := f.Stat()
	if err != nil || st.Mode().Perm() != 0o600 {
		t.Fatalf("log permissions: %v, %v", st, err)
	}
}

func TestStartFailureRecovery(t *testing.T) {
	c := testnet.StartControl(t, testnet.ControlOptions{RequireAuth: true})
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "tailscaled.state"), []byte("invalid state JSON"), 0o600); err != nil {
		t.Fatal(err)
	}
	n := New()
	t.Cleanup(func() { n.Stop() })
	cfg := Config{Dir: dir, Hostname: "rs-app-a1", ControlURL: c.URL, DisableNetns: true}
	if err := n.Start(cfg); CodeOf(err) != CodeIO {
		t.Fatalf("corrupt store Start: %v", err)
	}
	if err := n.Stop(); err != nil {
		t.Fatalf("Stop after failed Start: %v", err)
	}
	if _, _, err := n.NextEvent(0, 4096); CodeOf(err) != CodeClosed {
		t.Fatalf("events after failed Start: %v", err)
	}
	cfg.Dir = t.TempDir()
	if err := n.Start(cfg); err != nil {
		t.Fatalf("Start after failed Start: %v", err)
	}
}

func TestLifecycleBeforeStart(t *testing.T) {
	n := New()
	ctx := context.Background()
	for _, op := range []func(context.Context) error{n.Login, n.Logout, n.Rebind} {
		if err := op(ctx); CodeOf(err) != CodeState {
			t.Fatalf("operation before start: %v", err)
		}
	}
	if err := n.Start(Config{}); CodeOf(err) != CodeInvalid {
		t.Fatalf("invalid Start: %v", err)
	}
}
