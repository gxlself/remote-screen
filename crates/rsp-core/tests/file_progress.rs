use std::sync::Arc;

use rsp_core::file_progress::{
    CancelFlag, Direction, FailReason, Transfer, TransferError, TransferProgress, TransferState,
};

const SIZE: u64 = 1_000;

fn transfer(size: u64) -> Transfer {
    Transfer::new("transfer-1".into(), Direction::Send, size)
}

fn state(transfer: &Transfer) -> TransferState {
    transfer.progress().state
}

fn assert_invalid(
    transfer: &mut Transfer,
    op: &'static str,
    transition: impl FnOnce(&mut Transfer) -> Result<(), TransferError>,
) {
    let before = transfer.progress();
    let result = transition(transfer);
    assert_eq!(
        result,
        Err(TransferError::InvalidTransition {
            from: before.state.clone(),
            op,
        })
    );
    assert_eq!(transfer.progress(), before);
}

#[test]
fn legal_path_reports_every_state_and_always_reports_completion_advance() {
    let mut transfer = transfer(SIZE);
    assert_eq!(
        transfer.progress(),
        TransferProgress {
            transfer_id: "transfer-1".into(),
            direction: Direction::Send,
            state: TransferState::Queued,
            done: 0,
            size: SIZE,
            bytes_per_sec: 0,
            fail: None,
        }
    );

    let event = transfer.start(0, 0).expect("start from queued is valid");
    assert_eq!(event.state, TransferState::Running);
    assert_eq!(event.done, 0);

    assert_eq!(
        transfer.advance(600, 50_000).expect("advance is valid"),
        None
    );
    assert_eq!(
        transfer.advance(300, 99_000).expect("advance is valid"),
        None
    );

    let event = transfer
        .advance(100, 110_000)
        .expect("completing advance is valid")
        .expect("reaching size always emits an event");
    assert_eq!(event.done, SIZE);
    assert_eq!(event.state, TransferState::Running);

    let event = transfer.verifying().expect("verifying at size is valid");
    assert_eq!(event.state, TransferState::Verifying);
    assert_eq!(event.done, SIZE);

    let event = transfer
        .complete()
        .expect("complete from verifying is valid");
    assert_eq!(event.state, TransferState::Completed);
    assert_eq!(event.done, SIZE);
    assert_eq!(event.fail, None);
}

#[test]
fn resume_can_move_done_backwards_then_finish() {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("initial start is valid");
    let event = transfer
        .advance(600, 100_000)
        .expect("advance is valid")
        .expect("100 ms has elapsed");
    assert_eq!(event.done, 600);

    let event = transfer
        .interrupt(200_000)
        .expect("interrupt from running is valid");
    assert_eq!(event.state, TransferState::Interrupted);
    assert_eq!(event.done, 600);

    let event = transfer
        .start(400, 300_000)
        .expect("resume from interrupted is valid");
    assert_eq!(event.state, TransferState::Running);
    assert_eq!(event.done, 400);
    assert_eq!(event.bytes_per_sec, 0);

    let event = transfer
        .advance(600, 400_000)
        .expect("advance is valid")
        .expect("reaching size always emits an event");
    assert_eq!(event.done, SIZE);
    transfer.verifying().expect("verifying at size is valid");
    let event = transfer
        .complete()
        .expect("complete from verifying is valid");
    assert_eq!(event.state, TransferState::Completed);
}

#[test]
fn queued_rejects_running_only_transitions() {
    let mut transfer = transfer(SIZE);
    assert_invalid(&mut transfer, "advance", |transfer| {
        transfer.advance(1, 0).map(|_| ())
    });
    assert_invalid(&mut transfer, "interrupt", |transfer| {
        transfer.interrupt(0).map(|_| ())
    });
    assert_invalid(&mut transfer, "verifying", |transfer| {
        transfer.verifying().map(|_| ())
    });
    assert_invalid(&mut transfer, "complete", |transfer| {
        transfer.complete().map(|_| ())
    });
    assert_eq!(state(&transfer), TransferState::Queued);
}

#[test]
fn running_rejects_verifying_before_all_bytes_arrive() {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    transfer.advance(999, 0).expect("advance is valid");
    assert_invalid(&mut transfer, "verifying", |transfer| {
        transfer.verifying().map(|_| ())
    });
    assert_eq!(state(&transfer), TransferState::Running);
    assert_eq!(transfer.progress().done, 999);
}

#[test]
fn verifying_rejects_advance() {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    transfer.advance(SIZE, 0).expect("advance is valid");
    transfer.verifying().expect("verifying is valid");
    assert_invalid(&mut transfer, "advance", |transfer| {
        transfer.advance(0, 100_000).map(|_| ())
    });
    assert_eq!(state(&transfer), TransferState::Verifying);
}

fn completed_transfer() -> Transfer {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    transfer.advance(SIZE, 0).expect("advance is valid");
    transfer.verifying().expect("verifying is valid");
    transfer.complete().expect("complete is valid");
    transfer
}

fn failed_transfer() -> Transfer {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    transfer
        .fail(FailReason::Io)
        .expect("fail from running is valid");
    transfer
}

fn cancelled_transfer() -> Transfer {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    transfer.cancel().expect("cancel from running is valid");
    transfer
}

#[test]
fn terminal_states_reject_all_transitions() {
    for (initial_state, mut transfer) in [
        (TransferState::Completed, completed_transfer()),
        (TransferState::Failed, failed_transfer()),
        (TransferState::Cancelled, cancelled_transfer()),
    ] {
        assert_eq!(state(&transfer), initial_state);

        assert_invalid(&mut transfer, "start", |transfer| {
            transfer.start(0, 100_000).map(|_| ())
        });
        assert_invalid(&mut transfer, "advance", |transfer| {
            transfer.advance(0, 100_000).map(|_| ())
        });
        assert_invalid(&mut transfer, "interrupt", |transfer| {
            transfer.interrupt(100_000).map(|_| ())
        });
        assert_invalid(&mut transfer, "verifying", |transfer| {
            transfer.verifying().map(|_| ())
        });
        assert_invalid(&mut transfer, "complete", |transfer| {
            transfer.complete().map(|_| ())
        });
        assert_invalid(&mut transfer, "cancel", |transfer| {
            transfer.cancel().map(|_| ())
        });
        assert_invalid(&mut transfer, "fail", |transfer| {
            transfer.fail(FailReason::Protocol).map(|_| ())
        });
        assert_eq!(state(&transfer), initial_state);
    }
}

#[test]
fn advance_beyond_size_is_overflow_and_leaves_progress_unchanged() {
    let mut transfer = transfer(100);
    transfer.start(0, 0).expect("start is valid");
    transfer.advance(60, 0).expect("advance is valid");
    let before = transfer.progress();

    assert_eq!(
        transfer.advance(41, 100_000),
        Err(TransferError::Overflow {
            done: 60,
            add: 41,
            size: 100,
        })
    );
    assert_eq!(transfer.progress(), before);
    assert_eq!(before.done, 60);
    assert_eq!(before.state, TransferState::Running);
}

#[test]
fn start_beyond_size_is_rejected_without_changing_state() {
    let mut transfer = transfer(SIZE);
    assert_eq!(
        transfer.start(SIZE + 1, 0),
        Err(TransferError::OffsetBeyondSize {
            offset: SIZE + 1,
            size: SIZE,
        })
    );
    assert_eq!(state(&transfer), TransferState::Queued);
    assert_eq!(transfer.progress().done, 0);

    transfer.start(0, 0).expect("start is valid");
    transfer.interrupt(100_000).expect("interrupt is valid");
    let before = transfer.progress();
    assert_eq!(
        transfer.start(SIZE + 1, 200_000),
        Err(TransferError::OffsetBeyondSize {
            offset: SIZE + 1,
            size: SIZE,
        })
    );
    assert_eq!(transfer.progress(), before);
}

#[test]
fn advance_events_are_throttled_to_one_per_hundred_milliseconds() {
    let mut transfer = transfer(1_000_000);
    transfer.start(0, 0).expect("start is valid");

    assert_eq!(transfer.advance(1, 50_000).expect("advance is valid"), None);
    assert_eq!(transfer.advance(1, 99_000).expect("advance is valid"), None);
    let event = transfer
        .advance(1, 100_000)
        .expect("advance is valid")
        .expect("100 ms since the last event");
    assert_eq!(event.done, 3);
    assert_eq!(
        transfer.advance(1, 150_000).expect("advance is valid"),
        None
    );
    let event = transfer
        .advance(1, 200_000)
        .expect("advance is valid")
        .expect("100 ms since the last event");
    assert_eq!(event.done, 5);
}

#[test]
fn rate_uses_a_one_second_sliding_window_and_tolerates_clock_rollback() {
    const CHUNK: u64 = 104_857;
    const EXPECTED_RATE: u64 = 1_048_570;

    let mut transfer = transfer(10_000_000);
    transfer.start(0, 0).expect("start is valid");
    let mut event = None;
    for step in 1..=10 {
        event = transfer
            .advance(CHUNK, step * 100_000)
            .expect("advance is valid");
    }
    let event = event.expect("the one second boundary emits an event");
    assert_eq!(event.done, EXPECTED_RATE);
    assert!(
        event.bytes_per_sec.abs_diff(EXPECTED_RATE) <= EXPECTED_RATE / 100,
        "rate {} is outside 1% of {EXPECTED_RATE}",
        event.bytes_per_sec
    );

    let rate_before_rollback = event.bytes_per_sec;
    let rolled_back = transfer
        .advance(CHUNK, 500_000)
        .expect("a backwards clock must not panic");
    assert_eq!(rolled_back, None);
    assert_eq!(transfer.progress().bytes_per_sec, rate_before_rollback);

    let recovered = transfer
        .advance(CHUNK, 1_100_000)
        .expect("advance after clock recovery is valid");
    assert!(recovered.is_some());
}

#[test]
fn cancel_is_available_from_every_non_terminal_state() {
    let mut queued = transfer(SIZE);
    assert_eq!(
        queued.cancel().expect("cancel from queued is valid").state,
        TransferState::Cancelled
    );

    let mut running = transfer(SIZE);
    running.start(0, 0).expect("start is valid");
    assert_eq!(
        running
            .cancel()
            .expect("cancel from running is valid")
            .state,
        TransferState::Cancelled
    );

    let mut interrupted = transfer(SIZE);
    interrupted.start(0, 0).expect("start is valid");
    interrupted.interrupt(100_000).expect("interrupt is valid");
    assert_eq!(
        interrupted
            .cancel()
            .expect("cancel from interrupted is valid")
            .state,
        TransferState::Cancelled
    );

    let mut verifying = transfer(SIZE);
    verifying.start(0, 0).expect("start is valid");
    verifying.advance(SIZE, 0).expect("advance is valid");
    verifying.verifying().expect("verifying is valid");
    assert_eq!(
        verifying
            .cancel()
            .expect("cancel from verifying is valid")
            .state,
        TransferState::Cancelled
    );
}

#[test]
fn fail_records_the_reason_in_the_snapshot() {
    let mut transfer = transfer(SIZE);
    transfer.start(0, 0).expect("start is valid");
    let event = transfer
        .fail(FailReason::Sha256Mismatch)
        .expect("fail from running is valid");
    assert_eq!(event.state, TransferState::Failed);
    assert_eq!(event.fail, Some(FailReason::Sha256Mismatch));
    assert_eq!(transfer.progress().fail, Some(FailReason::Sha256Mismatch));
}

#[test]
fn cancel_flag_clones_share_one_state_across_threads() {
    let flag = CancelFlag::new();
    let shared = flag.clone();
    assert!(!flag.is_cancelled());
    assert!(!shared.is_cancelled());

    std::thread::spawn(move || shared.cancel())
        .join()
        .expect("cancel thread must not panic");
    assert!(flag.is_cancelled());
}

#[test]
fn transfer_ids_and_directions_are_preserved() {
    let transfer_id = "receive-42".to_owned();
    let transfer = Transfer::new(transfer_id.clone(), Direction::Receive, 42);
    let progress = Arc::new(transfer.progress());
    assert_eq!(progress.transfer_id, transfer_id);
    assert_eq!(progress.direction, Direction::Receive);
}
