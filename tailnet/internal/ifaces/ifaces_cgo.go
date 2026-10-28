//go:build (linux || android) && cgo

package ifaces

/*
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>

static void *rs_addr_bytes(struct sockaddr *addr) {
    if (addr->sa_family == AF_INET)
        return &((struct sockaddr_in *)addr)->sin_addr;
    return &((struct sockaddr_in6 *)addr)->sin6_addr;
}
*/
import "C"

import (
	"fmt"
	"net"
	"sort"
	"unsafe"

	"tailscale.com/net/netmon"
)

func init() {
	getImpl = getifaddrs
	register()
}

func getifaddrs() ([]netmon.Interface, error) {
	var head *C.struct_ifaddrs
	if rc, err := C.getifaddrs(&head); rc != 0 {
		return nil, fmt.Errorf("ifaces: getifaddrs: %w", err)
	}
	defer C.freeifaddrs(head)
	byName := make(map[string]*netmon.Interface)
	for p := head; p != nil; p = p.ifa_next {
		if p.ifa_name == nil {
			continue
		}
		name := C.GoString(p.ifa_name)
		iface := byName[name]
		if iface == nil {
			iface = &netmon.Interface{
				Interface: &net.Interface{Name: name, Index: int(C.if_nametoindex(p.ifa_name))},
				AltAddrs:  []net.Addr{},
			}
			byName[name] = iface
		}
		for _, flag := range []struct {
			c      C.uint
			goFlag net.Flags
		}{
			{C.IFF_UP, net.FlagUp}, {C.IFF_LOOPBACK, net.FlagLoopback},
			{C.IFF_MULTICAST, net.FlagMulticast}, {C.IFF_POINTOPOINT, net.FlagPointToPoint},
			{C.IFF_BROADCAST, net.FlagBroadcast},
		} {
			if p.ifa_flags&flag.c != 0 {
				iface.Flags |= flag.goFlag
			}
		}
		if p.ifa_addr == nil {
			continue
		}
		var size C.int
		switch p.ifa_addr.sa_family {
		case C.AF_INET:
			size = net.IPv4len
		case C.AF_INET6:
			size = net.IPv6len
		default:
			continue
		}
		// C.GoBytes copies the address before freeifaddrs releases its storage.
		ip := net.IP(C.GoBytes(unsafe.Pointer(C.rs_addr_bytes(p.ifa_addr)), size))
		mask := net.CIDRMask(int(size)*8, int(size)*8)
		if p.ifa_netmask != nil && p.ifa_netmask.sa_family == p.ifa_addr.sa_family {
			mask = net.IPMask(C.GoBytes(unsafe.Pointer(C.rs_addr_bytes(p.ifa_netmask)), size))
		}
		iface.AltAddrs = append(iface.AltAddrs, &net.IPNet{IP: ip, Mask: mask})
	}
	result := make([]netmon.Interface, 0, len(byName))
	for _, iface := range byName {
		result = append(result, *iface)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].Name < result[j].Name })
	return result, nil
}
