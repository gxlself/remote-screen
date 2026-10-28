package node

import (
	"bytes"
	"encoding/json"
	"io"
	"net/url"
	"path/filepath"
	"regexp"
	"strings"
)

type Config struct {
	Dir            string `json:"dir"`
	Hostname       string `json:"hostname"`
	ControlURL     string `json:"controlUrl"`
	AuthKey        string `json:"authKey"`
	Ephemeral      bool   `json:"ephemeral"`
	LogFile        string `json:"logFile"`
	Verbose        int    `json:"verbose"`
	DisableNetns   bool   `json:"disableNetns"`
	UDPIdleSeconds int    `json:"udpIdleSeconds"`
}

var hostnamePattern = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,62}$`)

func ParseConfig(b []byte) (Config, error) {
	d := json.NewDecoder(bytes.NewReader(b))
	d.DisallowUnknownFields()
	var cfg *Config
	if err := d.Decode(&cfg); err != nil {
		return Config{}, Errorf(CodeInvalid, "config: %v", err)
	}
	if cfg == nil {
		return Config{}, Errorf(CodeInvalid, "config: expected an object")
	}
	if err := d.Decode(new(any)); err != io.EOF {
		return Config{}, Errorf(CodeInvalid, "config: expected a single JSON object")
	}
	return normalizeConfig(*cfg)
}

func normalizeConfig(cfg Config) (Config, error) {
	if !filepath.IsAbs(cfg.Dir) || strings.ContainsRune(cfg.Dir, 0) {
		return Config{}, Errorf(CodeInvalid, "dir: must be an absolute path")
	}
	cfg.Hostname = strings.ToLower(cfg.Hostname)
	if !hostnamePattern.MatchString(cfg.Hostname) {
		return Config{}, Errorf(CodeInvalid, "hostname: must match %s", hostnamePattern.String())
	}
	u, err := url.Parse(cfg.ControlURL)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Hostname() == "" {
		return Config{}, Errorf(CodeInvalid, "controlUrl: must be an http(s) URL with a host")
	}
	if cfg.LogFile != "" && (!filepath.IsAbs(cfg.LogFile) || strings.ContainsRune(cfg.LogFile, 0)) {
		return Config{}, Errorf(CodeInvalid, "logFile: must be an absolute path")
	}
	if cfg.Verbose < 0 || cfg.Verbose > 2 {
		return Config{}, Errorf(CodeInvalid, "verbose: must be between 0 and 2")
	}
	if cfg.UDPIdleSeconds < 0 {
		return Config{}, Errorf(CodeInvalid, "udpIdleSeconds: must be nonnegative")
	}
	if cfg.UDPIdleSeconds == 0 {
		cfg.UDPIdleSeconds = 120
	}
	return cfg, nil
}
