// Package node owns one embedded tsnet node and its lifecycle.
package node

import (
	"context"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"time"

	_ "remotescreen/tailnet/internal/ifaces"

	"tailscale.com/client/local"
	"tailscale.com/envknob"
	"tailscale.com/ipn"
	"tailscale.com/logtail"
	"tailscale.com/net/netns"
	"tailscale.com/tsnet"
	"tailscale.com/types/logger"
)

var disableLogsOnce sync.Once

// Node is safe for concurrent use. It must not be copied after first use.
type Node struct {
	opMu sync.Mutex // serializes lifecycle operations, never taken by watch
	mu   sync.Mutex // protects the cached state and resource registry

	srv       *tsnet.Server
	lc        *local.Client
	cfg       Config
	state     ipn.State
	events    *eventQueue
	cancel    context.CancelFunc
	watchDone chan struct{}
	log       *rotatingLog
	tracked   map[uint64]io.Closer
	nextID    uint64
}

func New() *Node { return &Node{} }

func (n *Node) Start(cfg Config) error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	n.mu.Lock()
	started := n.srv != nil
	n.mu.Unlock()
	if started {
		return Errorf(CodeState, "already started")
	}
	cfg, err := normalizeConfig(cfg)
	if err != nil {
		return err
	}
	disableLogsOnce.Do(func() {
		logtail.Disable()
		envknob.SetNoLogsNoSupport()
	})
	if cfg.DisableNetns {
		netns.SetEnabled(false)
	}
	if err := os.MkdirAll(cfg.Dir, 0o700); err != nil {
		return operationError(CodeIO, "create dir", err)
	}
	log, err := openLog(cfg.LogFile)
	if err != nil {
		return operationError(CodeIO, "open logFile", err)
	}
	srv := &tsnet.Server{
		Dir: cfg.Dir, Hostname: cfg.Hostname, ControlURL: cfg.ControlURL,
		AuthKey: cfg.AuthKey, Ephemeral: cfg.Ephemeral,
		Logf: logger.Discard, UserLogf: log.logf,
	}
	if cfg.Verbose >= 1 {
		srv.Logf = log.logf
	}
	if err := srv.Start(); err != nil {
		// Early tsnet failures have no System and Close would dereference nil.
		// Its start failure pool already releases any partially built resources.
		if srv.Sys() != nil {
			srv.Close()
		}
		log.Close()
		return operationError(CodeIO, "start tsnet", err)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		srv.Close()
		log.Close()
		return operationError(CodeIO, "local client", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	q, done := newEventQueue(), make(chan struct{})
	n.mu.Lock()
	n.srv, n.lc, n.cfg = srv, lc, cfg
	n.state = ipn.NoState
	n.events, n.cancel, n.watchDone, n.log = q, cancel, done, log
	n.tracked = make(map[uint64]io.Closer)
	n.mu.Unlock()
	go n.watch(ctx, lc, q, done)
	return nil
}

func (n *Node) Stop() error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	n.mu.Lock()
	if n.srv == nil {
		n.mu.Unlock()
		return nil
	}
	srv, cancel, done, q, log := n.srv, n.cancel, n.watchDone, n.events, n.log
	tracked := n.tracked
	n.srv, n.lc, n.tracked = nil, nil, nil
	n.state = ipn.Stopped
	n.mu.Unlock()
	cancel()
	for _, c := range tracked {
		c.Close()
	}
	err := srv.Close()
	<-done
	log.Close()
	q.finish()
	if err != nil {
		return operationError(CodeIO, "stop tsnet", err)
	}
	return nil
}

func (n *Node) Login(ctx context.Context) error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	lc := n.localClient()
	if lc == nil {
		return Errorf(CodeState, "not started")
	}
	// Restart the local backend to discard an in-progress auth URL (otherwise
	// StartLoginInteractive reuses it). UpdatePrefs preserves Persist, and also
	// restores our ControlURL if a canceled Logout left an empty profile.
	if err := lc.Start(ctx, ipn.Options{UpdatePrefs: n.loginPrefs()}); err != nil {
		return operationError(CodeIO, "restart interactive login", err)
	}
	if err := lc.StartLoginInteractive(ctx); err != nil {
		return operationError(CodeIO, "login", err)
	}
	return nil
}

func (n *Node) Logout(ctx context.Context) error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	lc := n.localClient()
	if lc == nil {
		return Errorf(CodeState, "not started")
	}
	if err := lc.Logout(ctx); err != nil {
		return operationError(CodeIO, "logout", err)
	}
	// Logout deletes the active profile in v1.94.2, including ControlURL.
	// Recreate the logged-out profile immediately so the next interactive
	// login stays on this node's configured control server.
	if err := lc.Start(ctx, ipn.Options{UpdatePrefs: n.loginPrefs()}); err != nil {
		return operationError(CodeIO, "restore configuration after logout", err)
	}
	return nil
}

func (n *Node) loginPrefs() *ipn.Prefs {
	n.mu.Lock()
	cfg := n.cfg
	n.mu.Unlock()
	prefs := ipn.NewPrefs()
	prefs.ControlURL, prefs.Hostname, prefs.WantRunning = cfg.ControlURL, cfg.Hostname, true
	return prefs
}

func (n *Node) Rebind(ctx context.Context) error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	if _, _, err := n.running(); err != nil {
		return err
	}
	lc := n.localClient()
	for _, action := range []string{"rebind", "restun"} {
		if err := lc.DebugAction(ctx, action); err != nil {
			return fmt.Errorf("%s: %w", action, err)
		}
	}
	return nil
}

func (n *Node) localClient() *local.Client {
	n.mu.Lock()
	defer n.mu.Unlock()
	return n.lc
}

func (n *Node) running() (*tsnet.Server, Config, error) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.srv == nil {
		return nil, Config{}, Errorf(CodeState, "not started")
	}
	if n.state != ipn.Running {
		return nil, Config{}, Errorf(CodeNotRunning, "not running: %s", n.state)
	}
	return n.srv, n.cfg, nil
}

func (n *Node) track(c io.Closer) (untrack func()) {
	n.mu.Lock()
	if n.srv == nil {
		n.mu.Unlock()
		c.Close()
		return func() {}
	}
	n.nextID++
	id := n.nextID
	n.tracked[id] = c
	n.mu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			n.mu.Lock()
			delete(n.tracked, id)
			n.mu.Unlock()
		})
	}
}

func (n *Node) liveTracked() int {
	n.mu.Lock()
	defer n.mu.Unlock()
	return len(n.tracked)
}

const maxLogSize = 4 << 20

type rotatingLog struct {
	mu   sync.Mutex
	path string
	file *os.File
	size int64
}

func openLog(path string) (*rotatingLog, error) {
	l := &rotatingLog{path: path}
	if path == "" {
		return l, nil
	}
	if err := l.open(); err != nil {
		return nil, err
	}
	return l, nil
}

func (l *rotatingLog) open() error {
	f, err := os.OpenFile(l.path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return fmt.Errorf("open log: %w", err)
	}
	st, err := f.Stat()
	if err != nil {
		f.Close()
		return fmt.Errorf("stat log: %w", err)
	}
	l.file, l.size = f, st.Size()
	return nil
}

func (l *rotatingLog) logf(format string, args ...any) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.file == nil {
		return
	}
	// Both tsnet and controlclient log login URLs, including shortened URLs.
	// Keep the capability URL exclusively in the status/event API.
	lower := strings.ToLower(fmt.Sprintf(format, args...))
	if strings.Contains(lower, "/auth/") ||
		strings.Contains(lower, "authurl is") ||
		strings.Contains(lower, "received auth url:") ||
		strings.Contains(lower, "to start this tsnet server,") ||
		strings.Contains(lower, "newurl=http") ||
		strings.Contains(lower, "control says to open url") {
		format, args = "authUrl issued", nil
	}
	line := fmt.Sprintf("%d %s\n", time.Now().UnixMicro(), fmt.Sprintf(format, args...))
	if l.size+int64(len(line)) > maxLogSize {
		l.file.Close()
		l.file = nil
		if err := os.Rename(l.path, l.path+".1"); err != nil {
			return
		}
		if err := l.open(); err != nil {
			return
		}
	}
	written, _ := io.WriteString(l.file, line)
	l.size += int64(written)
}

func (l *rotatingLog) Close() error {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.file == nil {
		return nil
	}
	err := l.file.Close()
	l.file = nil
	return err
}
