package node

import (
	"context"
	"encoding/json"
	"sync"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
)

const eventCapacity = 256

// Each Start owns a fresh queue. Waiters on an earlier generation can drain it
// through its final stopped event even if the node has already been restarted.
type eventQueue struct {
	mu      sync.Mutex
	items   [][]byte
	changed chan struct{}
	closed  bool
}

func newEventQueue() *eventQueue {
	return &eventQueue{changed: make(chan struct{})}
}

func (q *eventQueue) signalLocked() {
	close(q.changed)
	q.changed = make(chan struct{})
}

func (q *eventQueue) push(ev any) {
	b, err := json.Marshal(ev)
	if err != nil {
		return
	}
	q.mu.Lock()
	defer q.mu.Unlock()
	if q.closed {
		return
	}
	q.appendLocked(b)
	q.signalLocked()
}

func (q *eventQueue) appendLocked(b []byte) {
	if len(q.items) == eventCapacity {
		copy(q.items, q.items[1:])
		q.items[len(q.items)-1] = b
	} else {
		q.items = append(q.items, b)
	}
}

func (q *eventQueue) close() {
	q.mu.Lock()
	defer q.mu.Unlock()
	if !q.closed {
		q.closed = true
		q.signalLocked()
	}
}

// finish makes stopped the final event even if another caller is emitting.
func (q *eventQueue) finish() {
	q.mu.Lock()
	defer q.mu.Unlock()
	if !q.closed {
		q.appendLocked([]byte(`{"type":"stopped"}`))
		q.closed = true
		q.signalLocked()
	}
}

func (q *eventQueue) next(timeout time.Duration, maxLen int) ([]byte, int, error) {
	if q == nil {
		return nil, 0, Errorf(CodeClosed, "event queue closed")
	}
	if maxLen < 0 {
		return nil, 0, Errorf(CodeInvalid, "maxLen: must be nonnegative")
	}
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	for {
		q.mu.Lock()
		if len(q.items) != 0 {
			b := q.items[0]
			if len(b) > maxLen {
				q.mu.Unlock()
				return nil, len(b), nil
			}
			q.items[0] = nil
			q.items = q.items[1:]
			q.mu.Unlock()
			return b, len(b), nil
		}
		if q.closed {
			q.mu.Unlock()
			return nil, 0, Errorf(CodeClosed, "event queue closed")
		}
		changed := q.changed
		q.mu.Unlock()
		select {
		case <-changed:
		case <-timer.C:
			return nil, 0, nil
		}
	}
}

func (n *Node) NextEvent(timeout time.Duration, maxLen int) (ev []byte, need int, err error) {
	n.mu.Lock()
	q := n.events
	n.mu.Unlock()
	return q.next(timeout, maxLen)
}

func (n *Node) emit(ev any) {
	n.mu.Lock()
	q := n.events
	n.mu.Unlock()
	if q != nil {
		q.push(ev)
	}
}

type stateEvent struct {
	Type    string  `json:"type"`
	State   string  `json:"state"`
	AuthURL *string `json:"authUrl"`
}

type typeEvent struct {
	Type string `json:"type"`
}

type authEvent struct {
	Type string `json:"type"`
	URL  string `json:"url"`
}

type errorEvent struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

func stringOrNil(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// watch consumes the bus in order; its timer is owned by this goroutine so
// cancellation cannot race a delayed netmap event past the stopped event.
func (n *Node) watch(ctx context.Context, lc *local.Client, q *eventQueue, done chan struct{}) {
	defer close(done)
	w, err := lc.WatchIPNBus(ctx, ipn.NotifyInitialState|ipn.NotifyInitialNetMap)
	if err != nil {
		if ctx.Err() == nil {
			q.push(errorEvent{Type: "error", Message: "watch IPN bus: " + err.Error()})
		}
		return
	}
	type result struct {
		notify ipn.Notify
		err    error
	}
	updates := make(chan result)
	readerDone := make(chan struct{})
	defer func() {
		w.Close()
		<-readerDone
	}()
	go func() {
		defer close(readerDone)
		for {
			v, err := w.Next()
			select {
			case updates <- result{v, err}:
			case <-ctx.Done():
				return
			}
			if err != nil {
				return
			}
		}
	}()
	var timer *time.Timer
	var timerC <-chan time.Time
	defer func() {
		if timer != nil {
			timer.Stop()
		}
	}()
	state, authURL := ipn.NoState, ""
	seenState := false
	seenURLs := make(map[string]bool)
	for {
		select {
		case <-ctx.Done():
			return
		case <-timerC:
			q.push(typeEvent{Type: "netmap"})
			timerC = nil
		case r := <-updates:
			if r.err != nil {
				if ctx.Err() == nil {
					q.push(errorEvent{Type: "error", Message: "watch IPN bus: " + r.err.Error()})
				}
				return
			}
			v := r.notify
			oldState, oldURL := state, authURL
			if v.State != nil {
				state = *v.State
				if state != ipn.NeedsLogin && state != ipn.NeedsMachineAuth {
					authURL = ""
				}
			}
			if v.BrowseToURL != nil {
				authURL = *v.BrowseToURL
				if authURL != "" && !seenURLs[authURL] {
					seenURLs[authURL] = true
					q.push(authEvent{Type: "authUrl", URL: authURL})
				}
			}
			if !seenState || oldState != state || oldURL != authURL {
				n.mu.Lock()
				n.state = state
				n.mu.Unlock()
				q.push(stateEvent{Type: "state", State: state.String(), AuthURL: stringOrNil(authURL)})
				seenState = true
			}
			if v.ErrMessage != nil && *v.ErrMessage != "" {
				q.push(errorEvent{Type: "error", Message: *v.ErrMessage})
			}
			if v.NetMap != nil {
				if timer == nil {
					timer = time.NewTimer(500 * time.Millisecond)
				} else {
					timer.Reset(500 * time.Millisecond)
				}
				timerC = timer.C
			}
		}
	}
}
