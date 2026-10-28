//go:build unix

package node

import (
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
)

func relaySocketpair(kind int) (int, *net.UnixConn, error) {
	// Darwin has no SOCK_CLOEXEC. The fork lock also protects the interval
	// before CloseOnExec when other Go goroutines start child processes.
	syscall.ForkLock.RLock()
	fds, err := syscall.Socketpair(syscall.AF_UNIX, kind, 0)
	if err == nil {
		for _, fd := range fds {
			syscall.CloseOnExec(fd)
		}
	}
	syscall.ForkLock.RUnlock()
	if err != nil {
		return -1, nil, err
	}
	if kind == syscall.SOCK_DGRAM {
		for _, fd := range fds {
			_ = syscall.SetsockoptInt(fd, syscall.SOL_SOCKET, syscall.SO_SNDBUF, 1<<20)
			_ = syscall.SetsockoptInt(fd, syscall.SOL_SOCKET, syscall.SO_RCVBUF, 4<<20)
		}
	}
	f := os.NewFile(uintptr(fds[1]), "tailnet-relay")
	c, err := net.FileConn(f)
	f.Close() // FileConn duplicates the descriptor and owns its copy.
	if err != nil {
		syscall.Close(fds[0])
		return -1, nil, err
	}
	u, ok := c.(*net.UnixConn)
	if !ok {
		c.Close()
		syscall.Close(fds[0])
		return -1, nil, fmt.Errorf("socketpair produced %T instead of UnixConn", c)
	}
	return fds[0], u, nil
}

type relay struct {
	unix    *net.UnixConn
	conn    net.Conn
	once    sync.Once
	done    chan struct{}
	untrack func()
}

func newRelay(unix *net.UnixConn, conn net.Conn) relay {
	return relay{unix: unix, conn: conn, done: make(chan struct{})}
}

func (r *relay) close(signal bool) error {
	r.once.Do(func() {
		if signal {
			// The Go-owned descriptor is nonblocking. Do not use UnixConn.Write:
			// a full caller receive queue must never block Stop or idle cleanup.
			if raw, err := r.unix.SyscallConn(); err == nil {
				_ = raw.Control(func(fd uintptr) { _, _ = syscall.Write(int(fd), nil) })
			}
		}
		r.unix.Close()
		r.conn.Close()
		close(r.done)
		if r.untrack != nil {
			r.untrack()
		}
	})
	return nil
}

type udpRelay struct {
	relay
	started time.Time
	last    atomic.Int64 // monotonic nanoseconds since started
}

func (r *udpRelay) Close() error { return r.close(true) }

func (r *udpRelay) upstream() {
	defer r.Close()
	buf := make([]byte, 64<<10)
	for {
		n, err := r.unix.Read(buf)
		if err != nil {
			return
		}
		if n == 0 {
			continue
		}
		r.last.Store(time.Since(r.started).Nanoseconds())
		if _, err := r.conn.Write(buf[:n]); errors.Is(err, net.ErrClosed) {
			return
		}
		// Transient send failures (including ENOBUFS) drop just this packet.
	}
}

func (r *udpRelay) downstream() {
	defer r.Close()
	buf := make([]byte, 64<<10)
	for {
		n, err := r.conn.Read(buf)
		if err != nil {
			return
		}
		if n == 0 {
			continue // Reserved exclusively for the local close notification.
		}
		r.last.Store(time.Since(r.started).Nanoseconds())
		if _, err := r.unix.Write(buf[:n]); err != nil {
			return
		}
	}
}

func (r *udpRelay) expire(idle time.Duration) {
	ticker := time.NewTicker(max(idle/4, 250*time.Millisecond))
	defer ticker.Stop()
	for {
		select {
		case <-r.done:
			return
		case <-ticker.C:
			if time.Since(r.started)-time.Duration(r.last.Load()) >= idle {
				r.Close()
				return
			}
		}
	}
}

type tcpRelay struct{ relay }

func (r *tcpRelay) Close() error { return r.close(false) }

func (r *tcpRelay) copy(dst, src net.Conn) {
	if _, err := io.Copy(dst, src); err != nil && !errors.Is(err, io.EOF) {
		r.Close()
		return
	}
	if cw, ok := dst.(interface{ CloseWrite() error }); ok {
		if err := cw.CloseWrite(); err == nil {
			return
		}
	}
	r.Close()
}

func (r *tcpRelay) run() {
	defer r.Close()
	done := make(chan struct{})
	go func() {
		defer close(done)
		r.copy(r.conn, r.unix)
	}()
	r.copy(r.unix, r.conn)
	<-done
}
