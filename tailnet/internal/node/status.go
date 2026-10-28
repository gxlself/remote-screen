package node

import (
	"context"
	"encoding/json"
	"fmt"
	"net/netip"
	"sort"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tailcfg"
)

type Status struct {
	State          string       `json:"state"`
	AuthURL        *string      `json:"authUrl"`
	Self           *SelfStatus  `json:"self"`
	MagicDNSSuffix string       `json:"magicDnsSuffix"`
	Peers          []PeerStatus `json:"peers"`
}

type SelfStatus struct {
	Hostname       string   `json:"hostname"`
	DNSName        string   `json:"dnsName"`
	StableID       string   `json:"stableId"`
	IPs            []string `json:"ips"`
	Online         bool     `json:"online"`
	DERPRegion     string   `json:"derpRegion"`
	DERPRegionName string   `json:"derpRegionName"`
}

type PeerStatus struct {
	Hostname        string   `json:"hostname"`
	DNSName         string   `json:"dnsName"`
	IPs             []string `json:"ips"`
	OS              string   `json:"os"`
	Online          bool     `json:"online"`
	Tags            []string `json:"tags"`
	Direct          bool     `json:"direct"`
	CurAddr         string   `json:"curAddr"`
	Relay           string   `json:"relay"`
	PeerRelay       string   `json:"peerRelay"`
	LastHandshakeUs uint64   `json:"lastHandshakeUs"`
	RxBytes         int64    `json:"rxBytes"`
	TxBytes         int64    `json:"txBytes"`
}

func (n *Node) StatusJSON(ctx context.Context) ([]byte, error) {
	lc := n.localClient()
	if lc == nil {
		return json.Marshal(statusFromIPN(nil, nil))
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	st, err := lc.Status(ctx)
	if err != nil {
		return nil, fmt.Errorf("status: %w", err)
	}
	dm, _ := lc.CurrentDERPMap(ctx)
	return json.Marshal(statusFromIPN(st, dm))
}

func statusFromIPN(st *ipnstate.Status, dm *tailcfg.DERPMap) Status {
	s := Status{State: "Stopped", Peers: []PeerStatus{}}
	if st == nil {
		return s
	}
	s.State, s.AuthURL = st.BackendState, stringOrNil(st.AuthURL)
	if st.CurrentTailnet != nil {
		s.MagicDNSSuffix = st.CurrentTailnet.MagicDNSSuffix
	}
	if p := st.Self; p != nil && len(p.TailscaleIPs) > 0 {
		s.Self = &SelfStatus{
			Hostname: p.HostName, DNSName: p.DNSName, StableID: string(p.ID), IPs: ipStrings(p.TailscaleIPs),
			Online: p.Online, DERPRegion: p.Relay,
		}
		if dm != nil {
			for _, r := range dm.Regions {
				if r != nil && r.RegionCode == p.Relay {
					s.Self.DERPRegionName = r.RegionName
					break
				}
			}
		}
	}
	for _, p := range st.Peer {
		if p == nil {
			continue
		}
		peer := PeerStatus{
			Hostname: p.HostName, DNSName: p.DNSName, IPs: ipStrings(p.TailscaleIPs),
			OS: p.OS, Online: p.Online, Tags: []string{},
			Direct: p.CurAddr != "", CurAddr: p.CurAddr, Relay: p.Relay, PeerRelay: p.PeerRelay,
			RxBytes: p.RxBytes, TxBytes: p.TxBytes,
		}
		if p.Tags != nil {
			peer.Tags = append(peer.Tags, p.Tags.AsSlice()...)
		}
		if !p.LastHandshake.IsZero() && p.LastHandshake.UnixMicro() > 0 {
			peer.LastHandshakeUs = uint64(p.LastHandshake.UnixMicro())
		}
		s.Peers = append(s.Peers, peer)
	}
	firstIP := func(p PeerStatus) string {
		if len(p.IPs) == 0 {
			return ""
		}
		return p.IPs[0]
	}
	sort.Slice(s.Peers, func(i, j int) bool {
		a, b := firstIP(s.Peers[i]), firstIP(s.Peers[j])
		if a == b {
			return s.Peers[i].DNSName < s.Peers[j].DNSName
		}
		return a < b
	})
	return s
}

func ipStrings(ips []netip.Addr) []string {
	s := make([]string, 0, len(ips))
	for _, ip := range ips {
		s = append(s, ip.String())
	}
	return s
}
