package testnet

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"testing"
)

var (
	serve       = flag.Bool("rs.serve", false, "serve a local test tailnet until stdin EOF or SIGTERM")
	requireAuth = flag.Bool("rs.require-auth", false, "require interactive authentication")
	authKey     = flag.String("rs.authkey", "", "required test auth key")
	forwardUDP  forwardFlags
	forwardTCP  forwardFlags
)

type forwardSpec struct {
	port   uint16
	target string
}

type forwardFlags []forwardSpec

func (f *forwardFlags) String() string {
	parts := make([]string, 0, len(*f))
	for _, spec := range *f {
		parts = append(parts, fmt.Sprintf("%d=%s", spec.port, spec.target))
	}
	return strings.Join(parts, ",")
}

func (f *forwardFlags) Set(s string) error {
	var specs []forwardSpec
	for _, part := range strings.Split(s, ",") {
		port, target, ok := strings.Cut(strings.TrimSpace(part), "=")
		n, err := strconv.ParseUint(port, 10, 16)
		if !ok || err != nil || n == 0 || target == "" {
			return fmt.Errorf("invalid forward %q: expected nonzero port=127.0.0.1:port", part)
		}
		specs = append(specs, forwardSpec{uint16(n), target})
	}
	*f = append(*f, specs...)
	return nil
}

func init() {
	flag.Var(&forwardUDP, "rs.forward-udp", "UDP forwards: port=127.0.0.1:port (repeatable or comma separated)")
	flag.Var(&forwardTCP, "rs.forward-tcp", "TCP forwards: port=127.0.0.1:port (repeatable or comma separated)")
}

func TestServe(t *testing.T) {
	if !*serve {
		t.Skip("pass -rs.serve to start the test tailnet helper")
	}
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM, os.Interrupt)
	defer signal.Stop(signals)
	c := StartControl(t, ControlOptions{RequireAuth: *requireAuth, AuthKey: *authKey})
	p := StartPeer(t, c, "rs-peer")
	p.UDPEcho(t, 7401)
	p.TCPEcho(t, 7402)
	for _, spec := range forwardUDP {
		p.ForwardUDP(t, spec.port, spec.target)
	}
	for _, spec := range forwardTCP {
		p.ForwardTCP(t, spec.port, spec.target)
	}
	ipStrings := make([]string, len(p.IPs))
	for i, ip := range p.IPs {
		ipStrings[i] = ip.String()
	}
	info := struct {
		ControlURL string `json:"controlUrl"`
		AdminURL   string `json:"adminUrl"`
		AuthKey    string `json:"authKey"`
		Peer       struct {
			Hostname string   `json:"hostname"`
			DNSName  string   `json:"dnsName"`
			IPs      []string `json:"ips"`
		} `json:"peer"`
		UDPEcho uint16 `json:"udpEcho"`
		TCPEcho uint16 `json:"tcpEcho"`
	}{ControlURL: c.URL, AdminURL: c.AdminURL, AuthKey: c.AuthKey, UDPEcho: 7401, TCPEcho: 7402}
	info.Peer.Hostname, info.Peer.DNSName, info.Peer.IPs = p.Hostname, p.DNSName, ipStrings
	b, err := json.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	fmt.Printf("RS_TESTNET %s\n", b)
	eof := make(chan struct{})
	go func() {
		io.Copy(io.Discard, os.Stdin)
		close(eof)
	}()
	select {
	case <-eof:
	case <-signals:
	}
}
