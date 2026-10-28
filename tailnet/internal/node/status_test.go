package node

import (
	"bytes"
	"context"
	"encoding/json"
	"net/netip"
	"os"
	"reflect"
	"testing"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/views"
)

func TestStatusFromIPN(t *testing.T) {
	for _, tc := range []struct {
		name  string
		st    *ipnstate.Status
		dm    *tailcfg.DERPMap
		check func(*testing.T, Status)
	}{
		{"stopped", nil, nil, func(t *testing.T, s Status) {
			if s.State != "Stopped" || s.Self != nil || s.AuthURL != nil || s.MagicDNSSuffix != "" || s.Peers == nil {
				t.Fatalf("stopped = %+v", s)
			}
		}},
		{"nil self", &ipnstate.Status{BackendState: "NeedsLogin", AuthURL: "http://127.0.0.1:35200/auth/test"}, nil, func(t *testing.T, s Status) {
			if s.Self != nil || s.AuthURL == nil || s.State != "NeedsLogin" {
				t.Fatalf("login = %+v", s)
			}
		}},
		{"self without IP", &ipnstate.Status{Self: &ipnstate.PeerStatus{}}, nil, func(t *testing.T, s Status) {
			if s.Self != nil {
				t.Fatal("self without IP must be null")
			}
		}},
		{"DERP region name", &ipnstate.Status{
			Self:           &ipnstate.PeerStatus{ID: "nT3stSelf1", TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.2")}, Relay: "test"},
			CurrentTailnet: &ipnstate.TailnetStatus{MagicDNSSuffix: "rs.test"},
		}, &tailcfg.DERPMap{Regions: map[int]*tailcfg.DERPRegion{1: {RegionCode: "test", RegionName: "Local test"}}}, func(t *testing.T, s Status) {
			if s.Self.DERPRegionName != "Local test" || s.Self.DERPRegion != "test" || s.Self.StableID != "nT3stSelf1" || s.MagicDNSSuffix != "rs.test" {
				t.Fatalf("region = %+v", s)
			}
		}},
		{"missing DERP map", &ipnstate.Status{Self: &ipnstate.PeerStatus{TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.2")}, Relay: "test"}}, nil, func(t *testing.T, s Status) {
			if s.Self.DERPRegionName != "" {
				t.Fatalf("region name = %q", s.Self.DERPRegionName)
			}
		}},
	} {
		t.Run(tc.name, func(t *testing.T) { tc.check(t, statusFromIPN(tc.st, tc.dm)) })
	}
	tags := views.SliceOf([]string{"tag:rs"})
	st := &ipnstate.Status{Peer: map[key.NodePublic]*ipnstate.PeerStatus{
		key.NewNode().Public(): {HostName: "direct", TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.2")}, CurAddr: "10.30.0.5:41641", Relay: "test", Tags: &tags, LastHandshake: time.UnixMicro(1761100000000000), RxBytes: 12, TxBytes: 34},
		key.NewNode().Public(): {HostName: "relay", TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.10")}, Relay: "test", PeerRelay: "10.30.0.6:41641"},
	}}
	s := statusFromIPN(st, nil)
	if len(s.Peers) != 2 || s.Peers[0].Hostname != "relay" || s.Peers[1].Hostname != "direct" {
		t.Fatalf("peers not sorted by IP string: %+v", s.Peers)
	}
	if p := s.Peers[0]; p.Direct || p.Relay != "test" || p.Tags == nil || p.LastHandshakeUs != 0 || p.PeerRelay == "" {
		t.Fatalf("relayed peer = %+v", p)
	}
	if p := s.Peers[1]; !p.Direct || p.CurAddr == "" || p.LastHandshakeUs != 1761100000000000 || p.RxBytes != 12 || p.TxBytes != 34 || !reflect.DeepEqual(p.Tags, []string{"tag:rs"}) {
		t.Fatalf("direct peer = %+v", p)
	}
}

func TestStatusFixture(t *testing.T) {
	b, err := os.ReadFile("../../testdata/status.json")
	if err != nil {
		t.Fatal(err)
	}
	d := json.NewDecoder(bytes.NewReader(b))
	d.DisallowUnknownFields()
	var s Status
	if err := d.Decode(&s); err != nil {
		t.Fatal(err)
	}
	out, err := json.Marshal(s)
	if err != nil {
		t.Fatal(err)
	}
	var want, got any
	if err := json.Unmarshal(b, &want); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(out, &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("fixture changed on roundtrip: %s", out)
	}
}

func TestStoppedStatus(t *testing.T) {
	b, err := New().StatusJSON(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if string(b) != `{"state":"Stopped","authUrl":null,"self":null,"magicDnsSuffix":"","peers":[]}` {
		t.Fatalf("stopped status = %s", b)
	}
}
