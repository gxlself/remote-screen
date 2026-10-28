package node

import (
	"context"
	"errors"
	"net"
	"os"
	"strings"
)

func classifyDialErr(err error) Code {
	if err == nil {
		return CodeOK
	}
	var e *Error
	if errors.As(err, &e) {
		return e.Code
	}
	s := strings.ToLower(err.Error())
	if errors.Is(err, context.DeadlineExceeded) || os.IsTimeout(err) || strings.Contains(s, "timed out") || strings.Contains(s, "timeout") {
		return CodeTimeout
	}
	// A name that does not resolve is unreachable however the resolver says
	// so: NXDOMAIN ("no such host") or SERVFAIL ("server misbehaving").
	var dnsErr *net.DNSError
	if errors.As(err, &dnsErr) {
		return CodeUnreachable
	}
	if strings.Contains(s, "refused") {
		return CodeRefused
	}
	for _, text := range []string{"no route", "unreachable", "no such host", "not found", "no address"} {
		if strings.Contains(s, text) {
			return CodeUnreachable
		}
	}
	return CodeIO
}
