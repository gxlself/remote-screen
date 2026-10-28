package node

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"testing"
)

func TestClassifyDialErr(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
		want Code
	}{
		{"nil", nil, CodeOK},
		{"deadline", context.DeadlineExceeded, CodeTimeout},
		{"wrapped deadline", fmt.Errorf("dial: %w", context.DeadlineExceeded), CodeTimeout},
		{"OS timeout", os.ErrDeadlineExceeded, CodeTimeout},
		{"io timeout", errors.New("i/o timeout"), CodeTimeout},
		{"timed out", errors.New("Connection TIMED OUT"), CodeTimeout},
		{"gonet refused", errors.New("connection was refused"), CodeRefused},
		{"no route", errors.New("no route to host"), CodeUnreachable},
		{"DNS", errors.New("lookup x: no such host"), CodeUnreachable},
		{"DNS server failure", &net.DNSError{Err: "server misbehaving", Name: "x", Server: "127.0.0.53:53", IsTemporary: true}, CodeUnreachable},
		{"DNS timeout", &net.DNSError{Err: "i/o timeout", Name: "x", IsTimeout: true}, CodeTimeout},
		{"unreachable", errors.New("network unreachable"), CodeUnreachable},
		{"not found", errors.New("name not found"), CodeUnreachable},
		{"no address", errors.New("no address for peer"), CodeUnreachable},
		{"node error", &Error{Code: CodeNotRunning}, CodeNotRunning},
		{"wrapped node error", fmt.Errorf("dial: %w", Errorf(CodeNotRunning, "timeout")), CodeNotRunning},
		{"other", errors.New("boom"), CodeIO},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := classifyDialErr(tc.err); got != tc.want {
				t.Fatalf("classifyDialErr(%v) = %d; want %d", tc.err, got, tc.want)
			}
		})
	}
}
