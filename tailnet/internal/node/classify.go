package node

import (
	"context"
	"errors"
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
