//go:build linux && cgo

package ifaces

import (
	"net"
	"testing"
)

func TestGet(t *testing.T) {
	got, err := Get()
	if err != nil {
		t.Fatal(err)
	}
	names := make(map[string]net.Flags)
	ips := make(map[string]bool)
	for _, iface := range got {
		names[iface.Name] = iface.Flags
		if iface.Index <= 0 {
			t.Errorf("interface %s has no index", iface.Name)
		}
		for _, addr := range iface.AltAddrs {
			ipnet, ok := addr.(*net.IPNet)
			if !ok {
				t.Fatalf("address is %T, want *net.IPNet", addr)
			}
			if ones, bits := ipnet.Mask.Size(); bits == 0 || ones > bits {
				t.Errorf("invalid prefix for %s", iface.Name)
			}
			ips[ipnet.IP.String()] = true
		}
	}
	std, err := net.Interfaces()
	if err != nil {
		t.Fatal(err)
	}
	for _, iface := range std {
		if _, ok := names[iface.Name]; iface.Flags&net.FlagUp != 0 && !ok {
			t.Errorf("missing UP interface %s", iface.Name)
		}
	}
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		t.Fatal(err)
	}
	for _, addr := range addrs {
		ipnet, ok := addr.(*net.IPNet)
		if ok && ipnet.IP.To4() != nil && !ips[ipnet.IP.String()] {
			t.Error("missing IPv4 interface address")
		}
	}
	if names["lo"]&net.FlagLoopback == 0 {
		t.Error("lo missing LOOPBACK flag")
	}
}
