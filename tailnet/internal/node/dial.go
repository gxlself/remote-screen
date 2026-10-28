//go:build unix

package node

import (
	"context"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode"

	"tailscale.com/tsnet"
)

func (n *Node) DialUDP(ip string, port uint16, timeout time.Duration) (fd int, err error) {
	addr, err := netip.ParseAddr(ip)
	if err != nil || addr.Zone() != "" || port == 0 {
		return -1, Errorf(CodeInvalid, "ip must be an IP literal without a zone and port must be nonzero")
	}
	srv, cfg, err := n.running()
	if err != nil {
		return -1, err
	}
	if timeout <= 0 {
		timeout = 5 * time.Second
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	conn, err := srv.Dial(ctx, "udp", netip.AddrPortFrom(addr, port).String())
	if err != nil {
		return -1, operationError(classifyDialErr(err), "dial UDP", err)
	}
	fd, unix, err := relaySocketpair(syscall.SOCK_DGRAM)
	if err != nil {
		conn.Close()
		return -1, operationError(CodeIO, "UDP socketpair", err)
	}
	r := &udpRelay{relay: newRelay(unix, conn), started: time.Now()}
	if err := n.registerRelay(srv, &r.relay, r); err != nil {
		syscall.Close(fd)
		return -1, err
	}
	// Avoid overflowing time.Duration for unusually large valid configs.
	seconds := min(int64(cfg.UDPIdleSeconds), int64((1<<63-1)/time.Second))
	go r.upstream()
	go r.downstream()
	go r.expire(time.Duration(seconds) * time.Second)
	return fd, nil
}

func (n *Node) DialTCP(host string, port uint16, timeout time.Duration) (fd int, err error) {
	if host == "" || strings.ContainsRune(host, 0) || strings.ContainsFunc(host, unicode.IsSpace) || port == 0 {
		return -1, Errorf(CodeInvalid, "host must be nonempty without whitespace and port must be nonzero")
	}
	srv, _, err := n.running()
	if err != nil {
		return -1, err
	}
	if timeout <= 0 {
		timeout = 10 * time.Second
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	conn, err := srv.Dial(ctx, "tcp", net.JoinHostPort(host, strconv.Itoa(int(port))))
	if err != nil {
		return -1, operationError(classifyDialErr(err), "dial TCP", err)
	}
	fd, unix, err := relaySocketpair(syscall.SOCK_STREAM)
	if err != nil {
		conn.Close()
		return -1, operationError(CodeIO, "TCP socketpair", err)
	}
	r := &tcpRelay{relay: newRelay(unix, conn)}
	if err := n.registerRelay(srv, &r.relay, r); err != nil {
		syscall.Close(fd)
		return -1, err
	}
	go r.run()
	return fd, nil
}

// Serialize only registration, never network I/O, with Start/Stop. A dial
// finishing after Stop must not register against a later Start generation.
func (n *Node) registerRelay(srv *tsnet.Server, r *relay, closer interface{ Close() error }) error {
	n.opMu.Lock()
	defer n.opMu.Unlock()
	current, _, err := n.running()
	if err == nil && current != srv {
		err = Errorf(CodeState, "node restarted during dial")
	}
	if err != nil {
		closer.Close()
		return err
	}
	r.untrack = n.track(closer)
	return nil
}
