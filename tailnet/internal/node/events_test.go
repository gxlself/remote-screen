package node

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
	"sync"
	"testing"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
	"tailscale.com/types/netmap"
)

func TestEventTimeout(t *testing.T) {
	n := &Node{events: newEventQueue()}
	start := time.Now()
	b, need, err := n.NextEvent(20*time.Millisecond, 4096)
	if b != nil || need != 0 || err != nil || time.Since(start) < 20*time.Millisecond {
		t.Fatalf("timeout = (%s, %d, %v)", b, need, err)
	}
}

func TestEventBufferRetry(t *testing.T) {
	n := &Node{events: newEventQueue()}
	n.emit(typeEvent{Type: "netmap"})
	want := []byte(`{"type":"netmap"}`)
	for range 2 {
		b, need, err := n.NextEvent(0, len(want)-1)
		if b != nil || need != len(want) || err != nil {
			t.Fatalf("short read = (%s, %d, %v)", b, need, err)
		}
	}
	b, need, err := n.NextEvent(0, len(want))
	if !bytes.Equal(b, want) || need != len(want) || err != nil {
		t.Fatalf("retry = (%s, %d, %v)", b, need, err)
	}
}

func TestEventDropOldest(t *testing.T) {
	n := &Node{events: newEventQueue()}
	for i := range 300 {
		n.emit(i)
	}
	for i := 44; i < 300; i++ {
		b, _, err := n.NextEvent(0, 4096)
		var got int
		if err != nil || json.Unmarshal(b, &got) != nil || got != i {
			t.Fatalf("event %d = %s (%v)", i, b, err)
		}
	}
	if b, _, err := n.NextEvent(0, 4096); b != nil || err != nil {
		t.Fatalf("after draining: %s, %v", b, err)
	}
}

func TestNeverStartedEvents(t *testing.T) {
	n := New()
	if _, _, err := n.NextEvent(time.Hour, 4096); CodeOf(err) != CodeClosed {
		t.Fatalf("NextEvent = %v", err)
	}
	if err := n.Stop(); err != nil {
		t.Fatal(err)
	}
}

func TestEventCloseWakesWaiters(t *testing.T) {
	q := newEventQueue()
	var wg sync.WaitGroup
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, _, err := q.next(time.Second, 4096); CodeOf(err) != CodeClosed {
				t.Errorf("closed waiter = %v", err)
			}
		}()
	}
	q.close()
	q.push(typeEvent{Type: "netmap"})
	wg.Wait()
}

func TestEventStoppedIsLast(t *testing.T) {
	q := newEventQueue()
	var wg sync.WaitGroup
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for range 300 {
				q.push(typeEvent{Type: "netmap"})
			}
		}()
	}
	q.finish()
	wg.Wait()
	for {
		b, _, err := q.next(0, 4096)
		if err != nil {
			t.Fatalf("before stopped: %v", err)
		}
		if string(b) == `{"type":"stopped"}` {
			break
		}
	}
	if _, _, err := q.next(0, 4096); CodeOf(err) != CodeClosed {
		t.Fatalf("after stopped: %v", err)
	}
}

func TestWatchEvents(t *testing.T) {
	updates := make(chan ipn.Notify)
	httpServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/localapi/v0/watch-ipn-bus" {
			http.NotFound(w, r)
			return
		}
		w.WriteHeader(http.StatusOK)
		w.(http.Flusher).Flush()
		for {
			select {
			case <-r.Context().Done():
				return
			case v := <-updates:
				if err := json.NewEncoder(w).Encode(v); err != nil {
					return
				}
				w.(http.Flusher).Flush()
			}
		}
	}))
	defer httpServer.Close()
	lc := &local.Client{Dial: func(ctx context.Context, _, _ string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "tcp", httpServer.Listener.Addr().String())
	}}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	n := &Node{events: newEventQueue()}
	done := make(chan struct{})
	go n.watch(ctx, lc, n.events, done)
	defer func() { cancel(); <-done }()
	send := func(v ipn.Notify) {
		select {
		case updates <- v:
		case <-ctx.Done():
			t.Fatal("watch send timed out")
		}
	}
	state, url := ipn.NeedsLogin, "http://127.0.0.1:35200/auth/first"
	send(ipn.Notify{State: &state, BrowseToURL: &url})
	if got := nextEvent(t, n, time.Second); got.Type != "authUrl" || got.URL != url {
		t.Fatalf("auth = %+v", got)
	}
	if got := nextEvent(t, n, time.Second); got.Type != "state" || got.State != "NeedsLogin" || got.AuthURL == nil || *got.AuthURL != url {
		t.Fatalf("state = %+v", got)
	}
	// Identical notifications produce neither duplicate state nor auth events.
	send(ipn.Notify{State: &state, BrowseToURL: &url})
	if b, _, err := n.NextEvent(20*time.Millisecond, 4096); b != nil || err != nil {
		t.Fatalf("duplicate = %s, %v", b, err)
	}
	url2 := "http://127.0.0.1:35200/auth/second"
	send(ipn.Notify{BrowseToURL: &url2})
	if got := nextEvent(t, n, time.Second); got.Type != "authUrl" || got.URL != url2 {
		t.Fatalf("second auth = %+v", got)
	}
	if got := nextEvent(t, n, time.Second); got.Type != "state" || got.AuthURL == nil || *got.AuthURL != url2 {
		t.Fatalf("same-state URL change = %+v", got)
	}
	// Coalesce a burst and verify the quiet period starts at the last map.
	send(ipn.Notify{NetMap: &netmap.NetworkMap{}})
	time.Sleep(100 * time.Millisecond)
	send(ipn.Notify{NetMap: &netmap.NetworkMap{}})
	if b, _, err := n.NextEvent(300*time.Millisecond, 4096); b != nil || err != nil {
		t.Fatalf("early netmap = %s, %v", b, err)
	}
	if got := nextEvent(t, n, time.Second); got.Type != "netmap" {
		t.Fatalf("netmap = %+v", got)
	}
	if b, _, err := n.NextEvent(20*time.Millisecond, 4096); b != nil || err != nil {
		t.Fatalf("extra netmap = %s, %v", b, err)
	}
	message := "invalid authkey"
	send(ipn.Notify{ErrMessage: &message})
	if got := nextEvent(t, n, time.Second); got.Type != "error" || got.Message != message {
		t.Fatalf("error = %+v", got)
	}
	running := ipn.Running
	send(ipn.Notify{State: &running})
	if got := nextEvent(t, n, time.Second); got.Type != "state" || got.State != "Running" || got.AuthURL != nil {
		t.Fatalf("running = %+v", got)
	}
	send(ipn.Notify{NetMap: &netmap.NetworkMap{}})
	cancel()
	<-done
	if b, _, err := n.NextEvent(550*time.Millisecond, 4096); b != nil || err != nil {
		t.Fatalf("event after cancel = %s, %v", b, err)
	}
}

func TestEventFixtures(t *testing.T) {
	f, err := os.Open("../../testdata/events.jsonl")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	seen := make(map[string]bool)
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		b := scanner.Bytes()
		var kind typeEvent
		if err := json.Unmarshal(b, &kind); err != nil {
			t.Fatal(err)
		}
		seen[kind.Type] = true
		var ev any
		switch kind.Type {
		case "state":
			ev = &stateEvent{}
		case "authUrl":
			ev = &authEvent{}
		case "netmap", "stopped":
			ev = &typeEvent{}
		case "error":
			ev = &errorEvent{}
		default:
			t.Fatalf("unknown event type %q", kind.Type)
		}
		d := json.NewDecoder(bytes.NewReader(b))
		d.DisallowUnknownFields()
		if err := d.Decode(ev); err != nil {
			t.Fatal(err)
		}
		out, err := json.Marshal(ev)
		if err != nil {
			t.Fatal(err)
		}
		var want, got any
		if err := json.Unmarshal(b, &want); err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(out, &got); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(want, got) {
			t.Fatalf("event changed on roundtrip: %s", out)
		}
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	if len(seen) != 5 {
		t.Fatalf("event types = %v", seen)
	}
}
