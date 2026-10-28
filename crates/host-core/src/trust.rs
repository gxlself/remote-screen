//! 信任日志验签、执行与同步。见 docs/dev/04-identity-security.md 信任声明。

use std::collections::{btree_map::Entry, BTreeMap};

use prost::Message;
use thiserror::Error;

use rsp_core::pb;
use rsp_core::trust::{
    self, Applied, BindNodeClaim, RejectReason, Rejected, TrustAction, TrustEntry, TrustError,
};

use crate::store::{
    consume_pending_bind, insert_pending_bind, record_bound_node, PairedDevice, PendingBind,
    RevokedDevice, Store, StoreError,
};

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Trust(#[from] TrustError),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: Vec<Applied>,
    pub rejected: Vec<Rejected>,
    pub revoked: Vec<[u8; 32]>,
}

/// Verify and apply a contiguous local trust-journal batch transactionally.
pub fn apply(store: &Store, user: &str, log: &[TrustEntry]) -> Result<ApplyReport, ApplyError> {
    let trust_state = store.trust_state()?;
    let (mut applied, mut rejected) =
        trust::apply_log(store, trust_state.last_applied_seq, user, log)?;
    let mut next = store.snapshot()?;
    let mut revoked = Vec::new();
    let mut approved = Vec::new();
    // Match apply_log's stable duplicate handling: only the first entry for a
    // sequence was examined. Keep the original signed statement bytes intact.
    let mut by_seq = BTreeMap::new();
    for signed in log {
        by_seq.entry(signed.seq).or_insert(signed);
    }
    for rejection in &rejected {
        if rejection.reason != RejectReason::UnknownSigner {
            continue;
        }
        let Some(signed) = by_seq.get(&rejection.seq) else {
            continue;
        };
        let Ok(statement) = pb::Statement::decode(signed.statement.as_slice()) else {
            continue;
        };
        let Ok(fp) = <[u8; 32]>::try_from(signed.signer_fp.as_slice()) else {
            continue;
        };
        if statement.kind == pb::statement::Kind::BindNode as i32
            && statement.subject_fp == fp
            && statement.user == user
            && !signed.signature.is_empty()
            && trust::valid_subject_node(&statement.subject_node)
        {
            insert_pending_bind(
                &mut next.pending,
                PendingBind {
                    fp,
                    signed: signed.encode_to_vec(),
                    issued_at_us: statement.issued_at_us,
                    source_seq: rejection.seq,
                },
            );
        }
    }
    applied.retain(|entry| {
        match &entry.action {
            TrustAction::Revoke { fp } => {
                next.paired.remove(fp);
                next.bound.remove(fp);
                next.pending.remove(fp);
                next.revoked.entry(*fp).or_insert_with(|| RevokedDevice {
                    fp: *fp,
                    revoked_at_us: rsp_core::clock::unix_micros(),
                    source_seq: entry.seq,
                });
                revoked.push(*fp);
            }
            TrustAction::RequireConfirm { fp } => {
                if let Some(device) = next.paired.get_mut(fp) {
                    device.require_confirm = true;
                }
            }
            TrustAction::Approve {
                fp,
                cert_der,
                name,
                permission,
            } => {
                if let Entry::Vacant(slot) = next.paired.entry(*fp) {
                    approved.push(*fp);
                    slot.insert(PairedDevice {
                        fp: *fp,
                        device_id: rsp_core::ids::id_from_fp(fp),
                        name: name.clone(),
                        cert_der: cert_der.clone(),
                        permission: *permission,
                        paired_at_us: rsp_core::clock::unix_micros(),
                        require_confirm: false,
                        platform: Default::default(),
                        paired_via: Default::default(),
                        whois_user: None,
                        whois_node: None,
                        whois_node_name: None,
                        path_permissions: Default::default(),
                    });
                }
            }
            TrustAction::BindNode {
                fp,
                node,
                issued_at_us,
            } => {
                if !next.paired.contains_key(fp)
                    || record_bound_node(
                        &mut next.bound,
                        BindNodeClaim {
                            fp: *fp,
                            node: node.clone(),
                            issued_at_us: *issued_at_us,
                        },
                        entry.seq,
                    )
                    .is_err()
                {
                    rejected.push(Rejected {
                        seq: entry.seq,
                        reason: RejectReason::StaleBinding,
                    });
                    return false;
                }
            }
        }
        true
    });
    for fp in approved {
        if let Some(device) = next.paired.get(&fp) {
            consume_pending_bind(device, &mut next.bound, &mut next.pending);
        }
    }
    rejected.sort_by_key(|entry| entry.seq);
    let max_processed = log
        .iter()
        .map(|entry| entry.seq)
        .filter(|seq| *seq > trust_state.last_applied_seq)
        .max()
        .unwrap_or(trust_state.last_applied_seq);
    next.trust.last_applied_seq = max_processed;
    store.replace_snapshot(next)?;
    Ok(ApplyReport {
        applied,
        rejected,
        revoked,
    })
}
