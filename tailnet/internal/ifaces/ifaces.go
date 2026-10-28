// Package ifaces supplies getifaddrs interfaces where Go's netlink queries are
// unavailable (notably Android 11 and later).
package ifaces

import (
	"errors"
	"runtime"

	"tailscale.com/net/netmon"
)

var getImpl func() ([]netmon.Interface, error)

func Get() ([]netmon.Interface, error) {
	if getImpl == nil {
		return nil, errors.New("ifaces: getifaddrs unavailable")
	}
	return getImpl()
}

// Called at the end of the cgo init, after getImpl has been installed.
func register() {
	if runtime.GOOS == "android" && getImpl != nil {
		netmon.RegisterInterfaceGetter(Get)
	}
}
