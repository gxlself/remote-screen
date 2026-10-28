use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::audit::{AuditError, AuditEvent, AuditQueue, MAX_EVENTS};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-h15-audit-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn event(ts: u64) -> AuditEvent {
    AuditEvent {
        ts,
        user: Some("alice".into()),
        device_id: None,
        host_id: None,
        server: None,
        event: "session_start".into(),
        path: None,
        duration_s: None,
        detail: None,
    }
}

#[test]
fn persists_restarts_and_removes_only_acknowledged_records() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(1)).unwrap();
    queue.push(event(2)).unwrap();
    let first = queue.batch(1).unwrap();
    queue.push(event(3)).unwrap();
    queue.acknowledge(&first).unwrap();
    drop(queue);
    let queue = AuditQueue::open(&dir.0).unwrap();
    assert_eq!(
        queue
            .batch(100)
            .unwrap()
            .items
            .iter()
            .map(|e| e.ts_us)
            .collect::<Vec<_>>(),
        [Some(2), Some(3)]
    );
    assert!(queue.path().ends_with("audit_queue.jsonl"));
    queue.acknowledge(&queue.batch(100).unwrap()).unwrap();
    assert!(AuditQueue::open(&dir.0).unwrap().is_empty());
}

#[test]
fn default_capacity_matches_spec_and_zero_capacity_is_rejected() {
    assert_eq!(MAX_EVENTS, 10_000);
    let dir = Dir::new();
    assert!(matches!(
        AuditQueue::open_with_capacity(&dir.0, 0),
        Err(AuditError::InvalidCapacity)
    ));
    assert!(!dir.0.exists());
}

#[test]
fn overflow_counts_survive_restart_and_inflight_ack_preserves_new_items() {
    const CAPACITY: usize = 200;
    let dir = Dir::new();
    let queue = AuditQueue::open_with_capacity(&dir.0, CAPACITY).unwrap();
    for i in 0..CAPACITY {
        queue.push(event(i as u64)).unwrap();
    }
    assert_eq!(queue.len(), CAPACITY);
    assert_eq!(queue.dropped_count(), 0);
    let batch = queue.batch(100).unwrap();
    assert_eq!(batch.items.len(), 100);
    for i in CAPACITY..CAPACITY + 150 {
        queue.push(event(i as u64)).unwrap();
    }
    assert_eq!(queue.len(), CAPACITY);
    assert_eq!(queue.dropped_count(), 150);
    queue.acknowledge(&batch).unwrap();
    assert_eq!(
        queue.len(),
        CAPACITY,
        "all in-flight records were already evicted"
    );
    drop(queue);
    let queue = AuditQueue::open_with_capacity(&dir.0, CAPACITY).unwrap();
    assert_eq!(queue.len(), CAPACITY);
    assert_eq!(queue.dropped_count(), 150);
    assert_eq!(queue.batch(1).unwrap().items[0].ts_us, Some(150));
    assert_eq!(
        queue.batch(CAPACITY).unwrap().items.last().unwrap().ts_us,
        Some(349)
    );
}

#[test]
fn reopening_with_different_capacities_preserves_drops_and_trims_oldest_records() {
    let dir = Dir::new();
    let queue = AuditQueue::open_with_capacity(&dir.0, 5).unwrap();
    for ts in 0..8 {
        queue.push(event(ts)).unwrap();
    }
    assert_eq!(queue.dropped_count(), 3);
    drop(queue);

    // The un-compacted journal still contains the three evicted pushes.
    let queue = AuditQueue::open_with_capacity(&dir.0, 8).unwrap();
    assert_eq!(queue.len(), 5);
    assert_eq!(queue.dropped_count(), 3);
    assert_eq!(queue.batch(1).unwrap().items[0].ts_us, Some(3));
    drop(queue);

    let queue = AuditQueue::open_with_capacity(&dir.0, 3).unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.dropped_count(), 5);
    assert_eq!(queue.batch(1).unwrap().items[0].ts_us, Some(5));
    queue.push(event(8)).unwrap();
    drop(queue);

    let queue = AuditQueue::open_with_capacity(&dir.0, 3).unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.dropped_count(), 6);
    assert_eq!(
        queue
            .batch(3)
            .unwrap()
            .items
            .iter()
            .map(|e| e.ts_us)
            .collect::<Vec<_>>(),
        [Some(6), Some(7), Some(8)]
    );
}

#[test]
fn shrinking_capacity_replays_pending_ack_before_counting_drops() {
    let dir = Dir::new();
    let queue = AuditQueue::open_with_capacity(&dir.0, 5).unwrap();
    for ts in 0..5 {
        queue.push(event(ts)).unwrap();
    }
    let batch = queue.batch(3).unwrap();
    let temp = queue.path().with_extension("jsonl.tmp");
    // The ack is durable even if its subsequent compaction fails.
    std::fs::create_dir(&temp).unwrap();
    assert!(queue.acknowledge(&batch).is_err());
    std::fs::remove_dir(&temp).unwrap();
    drop(queue);

    let queue = AuditQueue::open_with_capacity(&dir.0, 1).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.dropped_count(), 1);
    assert_eq!(queue.batch(1).unwrap().items[0].ts_us, Some(4));
}

#[test]
fn legacy_journal_without_capacity_can_be_reopened_and_resized() {
    use std::io::Write;
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("audit_queue.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "{{\"op\":\"state\",\"next_id\":4,\"dropped\":7}}").unwrap();
    for id in 1..=3 {
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({"op": "push", "id": id, "event": event(id)}),
        )
        .unwrap();
        writeln!(file).unwrap();
    }
    drop(file);

    let queue = AuditQueue::from_path(path).unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.dropped_count(), 7);
    drop(queue);

    let queue = AuditQueue::open_with_capacity(&dir.0, 2).unwrap();
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.dropped_count(), 8);
    assert_eq!(queue.batch(1).unwrap().items[0].ts_us, Some(2));
}

#[test]
fn repairs_torn_tail_but_rejects_complete_corrupt_records() {
    use std::io::Write;
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(1)).unwrap();
    let path = queue.path().to_path_buf();
    drop(queue);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"op\":")
        .unwrap();
    let queue = AuditQueue::open(&dir.0).unwrap();
    assert_eq!(queue.len(), 1);
    drop(queue);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"corrupt\n")
        .unwrap();
    assert!(AuditQueue::open(&dir.0).is_err());
}

#[cfg(unix)]
#[test]
fn spool_and_compaction_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(1)).unwrap();
    assert_eq!(
        std::fs::metadata(queue.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    queue.compact().unwrap();
    assert_eq!(
        std::fs::metadata(queue.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn failed_append_or_ack_preserves_memory_and_disk_until_retry() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    let path = queue.path().to_path_buf();
    let saved = path.with_extension("saved");
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(queue.push(event(1)).is_err());
    assert!(queue.is_empty());
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    queue.push(event(2)).unwrap();
    let batch = queue.batch(100).unwrap();
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(queue.acknowledge(&batch).is_err());
    assert_eq!(queue.len(), 1);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    queue.acknowledge(&batch).unwrap();
    assert!(AuditQueue::open(&dir.0).unwrap().is_empty());
}

#[test]
fn sequences_are_assigned_per_microsecond_across_event_kinds_and_retries() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    for (ts, kind) in [(5, "x"), (5, "y"), (7, "x"), (5, "z")] {
        queue
            .push(AuditEvent {
                event: kind.into(),
                ..event(ts)
            })
            .unwrap();
    }
    for _ in 0..2 {
        let batch = queue.batch(100).unwrap();
        assert_eq!(
            batch
                .items
                .iter()
                .map(|item| (item.ts_us, item.seq))
                .collect::<Vec<_>>(),
            [
                (Some(5), Some(0)),
                (Some(5), Some(1)),
                (Some(7), Some(0)),
                (Some(5), Some(2)),
            ]
        );
    }
}

#[test]
fn upload_preserves_microseconds_and_serializes_flat_seconds_and_sequence() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    let ts_us = 1_792_465_133_500_000;
    queue.push(event(ts_us)).unwrap();
    let batch = queue.batch(1).unwrap();
    let item = &batch.items[0];
    assert_eq!(item.ts, 1_792_465_133);
    assert_eq!(item.ts_us, Some(ts_us));
    assert_eq!(item.seq, Some(0));
    let value = serde_json::to_value(item).unwrap();
    assert_eq!(value["ts"], 1_792_465_133_u64);
    assert_eq!(value["ts_us"], ts_us);
    assert_eq!(value["seq"], 0);
    assert_eq!(value["event"], "session_start");
    assert!(value.get("record").is_none());
}

#[test]
fn restart_seeds_sequences_from_pending_entries_and_resets_after_all_acked() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(9)).unwrap();
    queue.push(event(9)).unwrap();
    drop(queue);

    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(9)).unwrap();
    let batch = queue.batch(100).unwrap();
    assert_eq!(
        batch.items.iter().map(|item| item.seq).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2)]
    );
    assert!(batch.items.iter().all(|item| item.ts_us == Some(9)));
    queue.acknowledge(&batch).unwrap();
    drop(queue);

    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(9)).unwrap();
    assert_eq!(queue.batch(1).unwrap().items[0].seq, Some(0));
}

#[test]
fn legacy_sequences_are_assigned_in_queue_order_and_persisted_by_compaction() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("audit_queue.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"op\":\"push\",\"id\":1,\"event\":{\"ts\":3,\"event\":\"x\"}}\n",
            "{\"op\":\"push\",\"id\":2,\"event\":{\"ts\":3,\"event\":\"x\"}}\n",
        ),
    )
    .unwrap();
    for _ in 0..2 {
        let queue = AuditQueue::open(&dir.0).unwrap();
        let batch = queue.batch(100).unwrap();
        assert_eq!(
            batch
                .items
                .iter()
                .map(|item| (item.ts_us, item.seq))
                .collect::<Vec<_>>(),
            [(Some(3), Some(0)), (Some(3), Some(1))]
        );
        let persisted: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(persisted[1]["seq"], 0);
        assert_eq!(persisted[2]["seq"], 1);
        assert_eq!(persisted[1]["event"]["ts"], 3);
    }
}

#[test]
fn replay_seeds_all_persisted_sequences_before_numbering_legacy_entries() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(
        dir.0.join("audit_queue.jsonl"),
        concat!(
            "{\"op\":\"push\",\"id\":1,\"event\":{\"ts\":3,\"event\":\"x\"}}\n",
            "{\"op\":\"push\",\"id\":2,\"seq\":7,\"event\":{\"ts\":3,\"event\":\"y\"}}\n",
            "{\"op\":\"push\",\"id\":3,\"seq\":4,\"event\":{\"ts\":3,\"event\":\"y\"}}\n",
            "{\"op\":\"push\",\"id\":4,\"event\":{\"ts\":3,\"event\":\"z\"}}\n",
        ),
    )
    .unwrap();
    let queue = AuditQueue::open(&dir.0).unwrap();
    queue.push(event(3)).unwrap();
    assert_eq!(
        queue
            .batch(100)
            .unwrap()
            .items
            .iter()
            .map(|item| item.seq)
            .collect::<Vec<_>>(),
        [Some(8), Some(7), Some(4), Some(9), Some(10)]
    );
}

#[test]
fn failed_append_does_not_consume_a_new_or_existing_sequence_counter() {
    let dir = Dir::new();
    let queue = AuditQueue::open(&dir.0).unwrap();
    let path = queue.path();
    let saved = path.with_extension("saved");
    for seq in 0..2 {
        std::fs::rename(path, &saved).unwrap();
        std::fs::create_dir(path).unwrap();
        assert!(matches!(queue.push(event(5)), Err(AuditError::Io(_))));
        assert_eq!(queue.len(), seq);
        std::fs::remove_dir(path).unwrap();
        std::fs::rename(&saved, path).unwrap();
        queue.push(event(5)).unwrap();
        assert_eq!(
            queue.batch(100).unwrap().items.last().unwrap().seq,
            Some(seq as u32)
        );
    }
}

#[test]
fn sequence_counters_expire_outside_the_latest_sixty_seconds() {
    let old_dir = Dir::new();
    let old_queue = AuditQueue::open(&old_dir.0).unwrap();
    for ts in [0, 61_000_000, 0] {
        old_queue.push(event(ts)).unwrap();
    }
    assert_eq!(old_queue.batch(100).unwrap().items[2].seq, Some(0));

    let recent_dir = Dir::new();
    let recent_queue = AuditQueue::open(&recent_dir.0).unwrap();
    for ts in [1, 30_000_000, 1] {
        recent_queue.push(event(ts)).unwrap();
    }
    assert_eq!(recent_queue.batch(100).unwrap().items[2].seq, Some(1));

    // Keep the exact lower boundary, and use the high-water timestamp even
    // when subsequent events arrive out of order.
    for ts in [60_000_001, 1, 0, 0] {
        recent_queue.push(event(ts)).unwrap();
    }
    let batch = recent_queue.batch(100).unwrap();
    assert_eq!(batch.items[4].seq, Some(2));
    assert_eq!(batch.items[5].seq, Some(0));
    assert_eq!(batch.items[6].seq, Some(0));
}

#[test]
fn eviction_and_acknowledgement_do_not_recycle_sequences_in_a_running_queue() {
    let dir = Dir::new();
    let queue = AuditQueue::open_with_capacity(&dir.0, 1).unwrap();
    queue.push(event(5)).unwrap();
    queue.push(event(7)).unwrap();
    queue.push(event(5)).unwrap();
    let batch = queue.batch(1).unwrap();
    assert_eq!(batch.items[0].seq, Some(1));
    assert_eq!(queue.dropped_count(), 2);
    queue.acknowledge(&batch).unwrap();
    queue.push(event(5)).unwrap();
    assert_eq!(queue.batch(1).unwrap().items[0].seq, Some(2));
}

#[test]
fn exhausted_sequence_rejects_push_without_changing_the_journal_or_queue() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("audit_queue.jsonl");
    std::fs::write(
        &path,
        format!(
            "{{\"op\":\"push\",\"id\":1,\"seq\":{},\"event\":{{\"ts\":3,\"event\":\"x\"}}}}\n",
            u32::MAX - 1
        ),
    )
    .unwrap();
    let queue = AuditQueue::open(&dir.0).unwrap();
    let before = std::fs::read(&path).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            queue.push(event(3)),
            Err(AuditError::SeqExhausted)
        ));
        assert_eq!(queue.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(queue.batch(1).unwrap().items[0].seq, Some(u32::MAX - 1));
    }
    queue.push(event(4)).unwrap();
    assert_eq!(queue.batch(100).unwrap().items[1].seq, Some(0));
}
