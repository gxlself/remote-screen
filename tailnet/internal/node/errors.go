package node

import (
	"context"
	"errors"
	"fmt"
)

type Code int32

const (
	CodeOK          Code = 0
	CodeInvalid     Code = -1
	CodeState       Code = -2
	CodeNotRunning  Code = -3
	CodeTimeout     Code = -4
	CodeRefused     Code = -5
	CodeUnreachable Code = -6
	CodeIO          Code = -7
	CodeClosed      Code = -8
)

type Error struct {
	Code Code
	Msg  string
}

func (e *Error) Error() string { return e.Msg }

func Errorf(code Code, format string, args ...any) *Error {
	return &Error{Code: code, Msg: fmt.Sprintf(format, args...)}
}

// Preserve the underlying error for errors.Is while giving ABI callers the
// operation's specified error code.
func operationError(code Code, operation string, err error) error {
	return fmt.Errorf("%w: %w", Errorf(code, "%s", operation), err)
}

func CodeOf(err error) Code {
	if err == nil {
		return CodeOK
	}
	var e *Error
	if errors.As(err, &e) {
		return e.Code
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return CodeTimeout
	}
	return CodeIO
}
