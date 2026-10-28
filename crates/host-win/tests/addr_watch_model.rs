use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::platform::Nic;
use host_win::addr_watch_model::{follow_nics, listen_signature, DEBOUNCE, MAX_DELAY, RESYNC};
use tokio::sync::mpsc::{self, Sender};
use tokio::task::{yield_now, JoinHandle};
use tokio::time::sleep;

fn nic(name: &str, addresses: &[&str], tailnet: bool) -> Nic {
    Nic {
        name: name.to_owned(),
        addresses: addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect(),
        networks: Vec::new(),
        mac: None,
        tailnet,
    }
}

fn changed_nics() -> Vec<Nic> {
    vec![nic("Tailscale", &["100.64.0.2"], true)]
}

#[derive(Default)]
struct Fixture {
    current: Arc<Mutex<Vec<Nic>>>,
    reads: Arc<AtomicUsize>,
    changes: Arc<Mutex<Vec<Vec<Nic>>>>,
}

impl Fixture {
    fn spawn(&self) -> (Sender<()>, JoinHandle<()>) {
        self.spawn_with_timing(DEBOUNCE, MAX_DELAY, RESYNC)
    }

    fn spawn_with_timing(
        &self,
        debounce: Duration,
        max_delay: Duration,
        resync: Duration,
    ) -> (Sender<()>, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(8);
        let current = self.current.clone();
        let reads = self.reads.clone();
        let changes = self.changes.clone();
        let task = tokio::spawn(follow_nics(
            rx,
            move || {
                reads.fetch_add(1, Ordering::SeqCst);
                current.lock().unwrap().clone()
            },
            move |nics| changes.lock().unwrap().push(nics),
            debounce,
            max_delay,
            resync,
        ));
        (tx, task)
    }

    fn read_count(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn changes(&self) -> Vec<Vec<Nic>> {
        self.changes.lock().unwrap().clone()
    }

    fn set(&self, nics: Vec<Nic>) {
        *self.current.lock().unwrap() = nics;
    }
}

#[test]
fn signature_normalizes_order_duplicates_and_ignores_unrelated_fields() {
    let original = vec![
        nic("Tailscale", &["100.64.0.2", "100.64.0.1"], true),
        nic("Ethernet", &["10.30.0.2"], false),
    ];
    let mut reordered = original.clone();
    reordered.reverse();
    reordered[1].addresses.reverse();
    reordered[1].addresses.push("100.64.0.1".parse().unwrap());
    reordered[1].mac = Some([2, 0, 0, 0, 0, 1]);
    reordered[1].networks = vec!["100.64.0.0/10".to_owned()];
    assert_eq!(listen_signature(&original), listen_signature(&reordered));

    let mut added = original.clone();
    added[0].addresses.push("100.64.0.3".parse().unwrap());
    assert_ne!(listen_signature(&original), listen_signature(&added));
    let mut retagged = original.clone();
    retagged[0].tailnet = false;
    assert_ne!(listen_signature(&original), listen_signature(&retagged));
    let mut renamed = original.clone();
    renamed[0].name = "Renamed adapter".to_owned();
    assert_ne!(listen_signature(&original), listen_signature(&renamed));
}

#[test]
fn signature_uses_addresses_and_tailnet_to_order_equal_names() {
    let original = vec![
        nic("Adapter", &["100.64.0.2"], true),
        nic("Adapter", &["100.64.0.1"], true),
        nic("Adapter", &["100.64.0.1"], false),
    ];
    let mut reordered = original.clone();
    reordered.reverse();
    assert_eq!(listen_signature(&original), listen_signature(&reordered));
    assert!(listen_signature(&[]).is_empty());
}

#[tokio::test(start_paused = true)]
async fn startup_reads_baseline_without_callback() {
    let fixture = Fixture::default();
    fixture.set(changed_nics());
    let (tx, task) = fixture.spawn();
    yield_now().await;
    assert_eq!(fixture.read_count(), 1);
    assert!(fixture.changes().is_empty());
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn one_event_waits_for_debounce_then_reports_current_nics() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    fixture.set(changed_nics());
    tx.send(()).await.unwrap();
    yield_now().await;
    sleep(Duration::from_millis(1400)).await;
    assert_eq!(fixture.read_count(), 1);
    assert!(fixture.changes().is_empty());
    sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.read_count(), 2);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn continuous_events_hit_max_delay_then_wait_for_quiet() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    fixture.set(changed_nics());
    tx.send(()).await.unwrap();
    yield_now().await;
    for second in 1..=10 {
        sleep(Duration::from_secs(1)).await;
        yield_now().await;
        if second < 6 {
            assert_eq!(fixture.read_count(), 1);
            assert!(fixture.changes().is_empty());
        } else {
            assert_eq!(fixture.read_count(), 2);
            assert_eq!(fixture.changes(), vec![changed_nics()]);
        }
        tx.send(()).await.unwrap();
        yield_now().await;
    }
    sleep(Duration::from_millis(1400)).await;
    assert_eq!(fixture.read_count(), 2);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.read_count(), 3);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unchanged_event_reads_without_callback() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    tx.send(()).await.unwrap();
    yield_now().await;
    sleep(Duration::from_millis(1600)).await;
    assert_eq!(fixture.read_count(), 2);
    assert!(fixture.changes().is_empty());
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn resync_detects_changes_without_events_and_ignores_unchanged_nics() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    fixture.set(changed_nics());
    sleep(RESYNC).await;
    yield_now().await;
    assert_eq!(fixture.read_count(), 2);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    sleep(RESYNC).await;
    yield_now().await;
    assert_eq!(fixture.read_count(), 3);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn resync_is_measured_from_the_most_recent_read() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    tx.send(()).await.unwrap();
    yield_now().await;
    sleep(Duration::from_millis(1600)).await;
    assert_eq!(fixture.read_count(), 2);
    fixture.set(changed_nics());
    sleep(Duration::from_millis(59_800)).await;
    assert_eq!(fixture.read_count(), 2);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.read_count(), 3);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn resync_also_caps_a_pending_debounce() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn_with_timing(
        Duration::from_secs(10),
        Duration::from_secs(20),
        Duration::from_secs(3),
    );
    yield_now().await;
    sleep(Duration::from_secs(2)).await;
    fixture.set(changed_nics());
    tx.send(()).await.unwrap();
    yield_now().await;
    sleep(Duration::from_millis(1100)).await;
    assert_eq!(fixture.read_count(), 2);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn closed_channel_exits_without_an_extra_read() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    drop(tx);
    yield_now().await;
    assert!(task.is_finished());
    task.await.unwrap();
    assert_eq!(fixture.read_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn closing_during_debounce_flushes_changes_before_returning() {
    let fixture = Fixture::default();
    let (tx, task) = fixture.spawn();
    yield_now().await;
    fixture.set(changed_nics());
    tx.send(()).await.unwrap();
    yield_now().await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(fixture.read_count(), 1);
    drop(tx);
    yield_now().await;
    assert!(task.is_finished());
    task.await.unwrap();
    assert_eq!(fixture.read_count(), 2);
    assert_eq!(fixture.changes(), vec![changed_nics()]);
}
