package node

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
)

func TestParseConfig(t *testing.T) {
	dir := t.TempDir()
	for _, tc := range []struct {
		name, field string
		value       any
		valid       bool
	}{
		{"missing dir", "dir", nil, false},
		{"relative dir", "dir", "state", false},
		{"uppercase hostname", "hostname", "RS-App-A1", true},
		{"missing hostname", "hostname", nil, false},
		{"underscore hostname", "hostname", "rs_app", false},
		{"long hostname", "hostname", strings.Repeat("a", 64), false},
		{"leading hyphen", "hostname", "-app", false},
		{"max hostname", "hostname", strings.Repeat("a", 63), true},
		{"ftp control", "controlUrl", "ftp://hs.example.com", false},
		{"no control host", "controlUrl", "https:///login", false},
		{"missing control", "controlUrl", nil, false},
		{"bad URL", "controlUrl", "http://%", false},
		{"negative idle", "udpIdleSeconds", -1, false},
		{"explicit idle", "udpIdleSeconds", 60, true},
		{"unknown", "foo", 1, false},
		{"relative log", "logFile", "logs", false},
		{"absolute log", "logFile", filepath.Join(dir, "log"), true},
		{"negative verbose", "verbose", -1, false},
		{"high verbose", "verbose", 3, false},
		{"wrong type", "verbose", "1", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			m := map[string]any{"dir": dir, "hostname": "rs-app", "controlUrl": "https://hs.example.com"}
			if tc.value == nil {
				delete(m, tc.field)
			} else {
				m[tc.field] = tc.value
			}
			b, err := json.Marshal(m)
			if err != nil {
				t.Fatal(err)
			}
			cfg, err := ParseConfig(b)
			if tc.valid {
				if err != nil {
					t.Fatal(err)
				}
				if cfg.Hostname != strings.ToLower(m["hostname"].(string)) {
					t.Fatalf("hostname was not normalized: %q", cfg.Hostname)
				}
				if tc.field != "udpIdleSeconds" && cfg.UDPIdleSeconds != 120 {
					t.Fatalf("idle default = %d", cfg.UDPIdleSeconds)
				}
			} else if CodeOf(err) != CodeInvalid || !strings.Contains(err.Error(), tc.field) {
				t.Fatalf("error = %v (code %d), want CodeInvalid mentioning %s", err, CodeOf(err), tc.field)
			}
		})
	}
	for _, b := range []string{"null", "[]", "{", `{ "dir": 3 }`, `{ } { }`, `{ } garbage`} {
		if _, err := ParseConfig([]byte(b)); CodeOf(err) != CodeInvalid {
			t.Errorf("ParseConfig(%q) = %v", b, err)
		}
	}
}

func TestCodeOf(t *testing.T) {
	for _, tc := range []struct {
		err  error
		code Code
	}{
		{nil, CodeOK}, {Errorf(CodeRefused, "refused %d", 7), CodeRefused},
		{fmt.Errorf("wrapped: %w", Errorf(CodeUnreachable, "route")), CodeUnreachable},
		{fmt.Errorf("wrapped: %w", context.DeadlineExceeded), CodeTimeout},
		{errors.New("io"), CodeIO},
	} {
		if got := CodeOf(tc.err); got != tc.code {
			t.Errorf("CodeOf(%v) = %d, want %d", tc.err, got, tc.code)
		}
	}
}
