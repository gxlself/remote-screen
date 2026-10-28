use std::net::SocketAddr;

use rs_mobile::rscore::{
    candidates::{addr_kind, host_target, manual_candidate, qr_candidates, TargetError},
    netinfo::{local_net, RawIface},
    store::{LastGoodRow, PairedHostRow},
};
use rsp_core::errors::ErrorCode;
use rsp_transport::dial::{plan, CandidateSource, IpNet, LocalNet, PathKind};

fn socket(value: &str) -> SocketAddr {
    value.parse().unwrap()
}

fn paired(addrs: &[&str]) -> PairedHostRow {
    PairedHostRow {
        host_id: "h".into(),
        fp: [7; 32],
        name: "Paired Mac".into(),
        addresses: addrs.iter().map(|value| socket(value)).collect(),
        paired_at_us: 1,
        permission: None,
    }
}

#[test]
fn unpaired_hosts_fail_before_using_other_candidate_sources() {
    let last_good = LastGoodRow {
        host_id: "h".into(),
        addr: socket("100.64.0.7:7400"),
        path: "tailnet".into(),
        transport: "quic".into(),
        at_us: 1,
    };
    let error = host_target("h", None, Some(&last_good), &LocalNet::default(), &[]).unwrap_err();
    assert_eq!(error, TargetError::NotPaired);
    assert_eq!(error.code(), ErrorCode::NotPaired);
    assert_eq!(error.code().code(), 181);
    assert_eq!(error.to_string(), "host is not paired");
}

#[test]
fn paired_addresses_keep_pinned_identity_and_avoid_lan_pruning() {
    let paired = paired(&["100.64.0.7:7400", "127.0.0.1:7400", "192.168.31.7:7400"]);
    let local = local_net(
        &[RawIface {
            name: "en0".into(),
            addrs: vec!["192.168.31.20/24".parse().unwrap()],
        }],
        &[],
    );
    let target = host_target("h", Some(&paired), None, &local, &[]).unwrap();
    assert_eq!(target.host_id, "h");
    assert_eq!(target.host_name, paired.name);
    assert_eq!(target.fp, paired.fp);
    assert_eq!(target.candidates.len(), 3);
    for ((candidate, addr), kind) in target.candidates.iter().zip(&paired.addresses).zip([
        PathKind::Tailnet,
        PathKind::Manual,
        PathKind::Manual,
    ]) {
        assert_eq!(candidate.addr, *addr);
        assert_eq!(candidate.kind, kind);
        assert_eq!(candidate.source, CandidateSource::Cache);
    }
    assert_eq!(target.observations.host_version_ok, None);
    assert_eq!(target.observations.local, local);
    assert!(target.observations.failure.is_none());
    assert!(target.observations.plan.candidates.is_empty());
    assert!(target.observations.plan.skipped.is_empty());
}

#[test]
fn last_good_preserves_original_path_and_wins_duplicate_addresses() {
    let paired = paired(&["100.64.0.7:7400", "10.30.1.5:7400"]);
    for (path, kind) in [
        ("intranet", PathKind::Intranet),
        ("tailnet", PathKind::Tailnet),
        ("lan", PathKind::Lan),
        ("manual", PathKind::Manual),
        ("unknown", PathKind::Manual),
    ] {
        let last_good = LastGoodRow {
            host_id: "h".into(),
            addr: socket("10.30.1.5:7400"),
            path: path.into(),
            transport: "quic".into(),
            at_us: 1,
        };
        let target = host_target(
            "h",
            Some(&paired),
            Some(&last_good),
            &LocalNet::default(),
            &[],
        )
        .unwrap();
        assert_eq!(target.candidates.len(), 2);
        assert_eq!(target.candidates[0].addr, last_good.addr);
        assert_eq!(
            target.candidates[0].kind,
            PathKind::LastGood(Box::new(kind))
        );
        assert_eq!(target.candidates[0].source, CandidateSource::Cache);
    }
}

#[test]
fn no_usable_addresses_maps_to_offline() {
    let paired = paired(&[]);
    let error = host_target("h", Some(&paired), None, &LocalNet::default(), &[]).unwrap_err();
    assert_eq!(error, TargetError::NoAddress);
    assert_eq!(error.code(), ErrorCode::Offline);
    assert_eq!(error.code().code(), 110);
    assert_eq!(error.to_string(), "no address for host");
}

#[test]
fn intranet_cidrs_are_the_settings_ranges_without_duplicates() {
    let paired = paired(&["100.64.0.7:7400"]);
    let settings: Vec<IpNet> = ["10.30.0.0/16", "192.168.31.0/24", "10.30.0.0/16"]
        .iter()
        .map(|net| net.parse().unwrap())
        .collect();
    let target = host_target("h", Some(&paired), None, &LocalNet::default(), &settings).unwrap();
    assert_eq!(
        target.intranet_cidrs,
        ["10.30.0.0/16", "192.168.31.0/24"]
            .iter()
            .map(|net| net.parse::<IpNet>().unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn dial_plan_keeps_paired_manual_addresses() {
    let paired = paired(&[
        "100.64.0.7:7400",
        "127.0.0.1:7400",
        "192.168.31.7:7400",
        "10.30.1.6:7400",
    ]);
    let local = local_net(
        &[RawIface {
            name: "en0".into(),
            addrs: vec!["192.168.31.20/24".parse().unwrap()],
        }],
        &[],
    );
    let target = host_target("h", Some(&paired), None, &local, &[]).unwrap();
    let plan = plan(target.candidates, &local, &target.intranet_cidrs);
    assert_eq!(plan.candidates.len(), 4);
    assert!(plan
        .candidates
        .iter()
        .all(|candidate| paired.addresses.contains(&candidate.addr)));
    assert!(plan.skipped.is_empty());
}

#[test]
fn qr_candidates_support_both_ip_families_and_skip_hostnames() {
    let candidates = qr_candidates(&[
        "100.64.0.7:7400".into(),
        "mac.local:7400".into(),
        "[fd7a:115c:a1e0::7]:7400".into(),
    ]);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].addr, socket("100.64.0.7:7400"));
    assert_eq!(candidates[1].addr, socket("[fd7a:115c:a1e0::7]:7400"));
    assert!(candidates
        .iter()
        .all(|candidate| candidate.source == CandidateSource::Manual
            && candidate.kind == PathKind::Tailnet));
    assert!(qr_candidates(&["invalid".into(), "100.64.0.7:bad".into()]).is_empty());
    assert!(qr_candidates(&[]).is_empty());
}

#[test]
fn manual_addresses_only_classify_tailnet_specially() {
    for (addr, expected) in [
        ("100.64.0.7:7400", PathKind::Tailnet),
        ("[fd7a:115c:a1e0::7]:7400", PathKind::Tailnet),
        ("192.168.31.7:7400", PathKind::Manual),
        ("10.30.1.5:7400", PathKind::Manual),
        ("127.0.0.1:7400", PathKind::Manual),
        ("[::1]:7400", PathKind::Manual),
    ] {
        let addr = socket(addr);
        assert_eq!(addr_kind(addr.ip()), expected);
        let candidate = manual_candidate(addr);
        assert_eq!(candidate.addr, addr);
        assert_eq!(candidate.kind, expected);
        assert_eq!(candidate.source, CandidateSource::Manual);
    }
}
