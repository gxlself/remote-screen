use rsp_core::clock::{rtt_median, ClockSample, PingSchedule};
use rsp_core::pb::{PathKind, Transport};
use rsp_core::stats::{
    feedback_from, ClientStatsCollector, DelayGradient, HostStatsCollector, Percentiles,
    SessionSummary,
};

const SECOND: u64 = 1_000_000;

fn clock_sample(rtt_us: u64) -> ClockSample {
    ClockSample {
        rtt_us,
        offset_us: 0,
    }
}

#[test]
fn nearest_rank_percentiles_are_stable() {
    let mut p = Percentiles::new(100);
    p.extend(1..=100);
    assert_eq!(p.p50(), 50);
    assert_eq!(p.p95(), 95);
    assert_eq!(p.p99(), 99);

    let mut even = Percentiles::new(10);
    even.extend([10, 20, 30, 40]);
    assert_eq!(even.p50(), 20);
    assert_eq!(even.p95(), 40);

    even.push(100);
    assert_eq!(even.len(), 5);
    assert_eq!(even.p95(), 100);
}

#[test]
fn fixed_percentile_window_drops_old_values() {
    let mut p = Percentiles::new(3);
    p.extend([1, 2, 3]);
    p.extend([100, 200, 300]);
    assert_eq!(p.len(), 3);
    assert_eq!(p.p50(), 200);
    assert_eq!(p.p95(), 300);
}

#[test]
fn rtt_median_uses_most_recent_samples() {
    let samples = [
        clock_sample(90),
        clock_sample(10),
        clock_sample(20),
        clock_sample(30),
        clock_sample(40),
        clock_sample(50),
    ];
    assert_eq!(rtt_median(&samples, 5), Some(30));
    assert_eq!(rtt_median(&samples, 1), Some(50));
    assert_eq!(rtt_median(&samples, 0), None);
    assert_eq!(rtt_median(&[], 5), None);

    let even = [clock_sample(10), clock_sample(20)];
    assert_eq!(rtt_median(&even, 2), Some(15));
}

#[test]
fn ping_schedule_is_fast_for_five_seconds_then_slow() {
    let start = 7 * SECOND;
    let mut schedule = PingSchedule::starting_at(start);
    let mut sent = Vec::new();
    let mut now = start;
    for _ in 0..30 {
        now = schedule.next_due(now);
        schedule.on_sent(now);
        sent.push(now);
    }

    // 0 through 5 s inclusive is 26 probes at 200 ms spacing. The task's
    // "about 25" wording differs only by whether the endpoint is counted.
    let first_phase: Vec<u64> = (0..26).map(|i| start + i * 200_000).collect();
    assert_eq!(&sent[..26], first_phase.as_slice());
    assert_eq!(sent[26], start + 6 * SECOND);
    assert_eq!(sent[29], start + 9 * SECOND);
    assert!(sent[26..]
        .windows(2)
        .all(|pair| pair[1] - pair[0] == SECOND));

    let mut lazy = PingSchedule::new();
    assert_eq!(lazy.next_due(start), start);
    lazy.on_sent(start);
    assert_eq!(lazy.next_due(start), start + 200_000);

    let mut late = PingSchedule::starting_at(0);
    late.on_sent(0);
    assert_eq!(late.next_due(0), 200_000);
    late.on_sent(5_000_000);
    assert_eq!(late.next_due(5_000_000), 6_000_000);
}

#[test]
fn sixty_fps_snapshot_reports_fps_and_bitrate() {
    let start = 10 * SECOND;
    let mut collector = ClientStatsCollector::starting_at(start);
    for frame in 0..30 {
        let now = start + frame * SECOND / 60;
        collector.on_frame_complete(now, 10_000, 5, 0, false);
        collector.on_decoded(now, 2_000, 20_000);
    }
    collector.set_path(PathKind::Lan, Transport::Quic);
    for rtt in [90, 10, 20, 30, 40, 50] {
        collector.on_rtt(rtt);
    }

    assert!(collector.snapshot(start + 400_000).is_none());
    let stats = collector.snapshot(start + 500_000).expect("2 Hz snapshot");
    assert_eq!(stats.fps, 60);
    assert_eq!(stats.recv_kbps, 4_800);
    assert_eq!(stats.frames_lost, 0);
    assert_eq!(stats.fec_recovered, 0);
    assert_eq!(stats.decode_p50_us, 2_000);
    assert_eq!(stats.decode_p95_us, 2_000);
    assert_eq!(stats.latency_p50_us, 20_000);
    assert_eq!(stats.latency_p95_us, 20_000);
    assert_eq!(stats.rtt_us, 30);
    assert_eq!(stats.path, PathKind::Lan);
    assert_eq!(stats.transport, Transport::Quic);
}

#[test]
fn snapshot_counts_losses_and_fec_recoveries() {
    let start = SECOND;
    let mut collector = ClientStatsCollector::starting_at(start);
    collector.on_frame_complete(start + 10_000, 1_000, 4, 1, true);
    collector.on_frame_complete(start + 20_000, 1_000, 4, 0, false);
    collector.on_frame_lost(start + 30_000);

    let stats = collector.snapshot(start + 500_000).expect("snapshot");
    assert_eq!(stats.frames_lost, 1);
    assert_eq!(stats.fec_recovered, 1);
    assert!((stats.loss_pct - 100.0 / 3.0).abs() < 0.01);
}

#[test]
fn snapshot_window_rolls_without_double_counting() {
    let start = SECOND;
    let mut collector = ClientStatsCollector::starting_at(start);
    for frame in 0..30 {
        let now = start + frame * SECOND / 60;
        collector.on_frame_complete(now, 10_000, 1, 0, false);
    }
    let first = collector.snapshot(start + 500_000).expect("first snapshot");
    assert_eq!(first.fps, 60);

    let empty = collector.snapshot(start + SECOND).expect("second snapshot");
    assert_eq!(empty.fps, 0);
    assert_eq!(empty.recv_kbps, 0);

    for frame in 0..30 {
        let now = start + SECOND + frame * SECOND / 60;
        collector.on_frame_complete(now, 5_000, 1, 0, false);
    }
    let third = collector
        .snapshot(start + SECOND + 500_000)
        .expect("third snapshot");
    assert_eq!(third.fps, 60);
    assert_eq!(third.recv_kbps, 2_400);
}

#[test]
fn session_summary_uses_whole_session_percentiles() {
    let mut collector = ClientStatsCollector::new();
    for value in 1..=100u64 {
        collector.on_decoded(value, 1_000, value * 1_000);
    }
    collector.on_frame_lost(101);
    assert_eq!(
        collector.summary(),
        SessionSummary {
            latency_p50_us: 50_000,
            latency_p95_us: 95_000,
            latency_p99_us: 99_000,
            frames_decoded: 100,
            frames_lost: 1,
        }
    );
}

#[test]
fn feedback_uses_trailing_two_hundred_milliseconds() {
    let mut collector = ClientStatsCollector::new();
    collector.on_frame_complete(0, 5_000, 3, 2, true);
    collector.on_frame_complete(100_000, 5_000, 3, 0, false);
    collector.on_frame_complete(150_000, 2_000, 2, 1, true);
    collector.on_frame_lost(180_000);
    collector.on_decoded(100_000, 4_000, 30_000);
    collector.on_decoded(150_000, 8_000, 50_000);
    collector.set_path(PathKind::TailnetDerp, Transport::Tcp);

    let feedback = feedback_from(&collector, 300_000, 7, 2, 1_234);
    assert_eq!(feedback.epoch, 7);
    assert_eq!(feedback.interval_ms, 200);
    assert_eq!(feedback.frames_received, 2);
    assert_eq!(feedback.frames_lost, 1);
    assert_eq!(feedback.frames_recovered, 1);
    assert_eq!(feedback.shards_received, 5);
    assert_eq!(feedback.shards_lost, 1);
    assert_eq!(feedback.recv_kbps, 280);
    assert_eq!(feedback.delay_gradient_us_per_s, 1_234);
    assert_eq!(feedback.owd_p50_us, 0);
    assert_eq!(feedback.decode_queue, 2);
    assert_eq!(feedback.decode_p50_us, 4_000);
    assert_eq!(feedback.render_fps, 10);
}

#[test]
fn delay_gradient_fits_linear_owd_growth() {
    let mut gradient = DelayGradient::new();
    for frame in 0..=60u64 {
        let at = frame * SECOND / 60;
        let owd = 20_000 + (at as i64 * 8_000) / SECOND as i64;
        gradient.push(at, owd);
    }
    let value = gradient.gradient_us_per_s(SECOND);
    assert!((value - 8_000).abs() <= 1, "got {value}");
}

#[test]
fn delay_gradient_is_near_zero_when_stable_and_ignores_old_samples() {
    let mut gradient = DelayGradient::new();
    for frame in 0..=60u64 {
        gradient.push(frame * SECOND / 60, 20_000);
    }
    assert_eq!(gradient.gradient_us_per_s(SECOND), 0);

    let mut rolling = DelayGradient::new();
    rolling.push(0, 1_000);
    rolling.push(SECOND, 10_000);
    rolling.push(SECOND + 1_000, 12_000);
    let value = rolling.gradient_us_per_s(SECOND + 1_000);
    assert!(value > 1_000_000, "got {value}");
}

#[test]
fn host_stats_are_emitted_once_per_second() {
    let start = 5 * SECOND;
    let mut host = HostStatsCollector::starting_at(start);
    for frame in 0..60 {
        host.on_captured(start + frame * SECOND / 60);
        host.on_encoded(2_000 + frame % 10);
        host.on_sent(10_000);
    }
    assert!(host.tick(start + 999_999, 17, PathKind::Lan, "").is_none());
    let stats = host
        .tick(start + SECOND, 17, PathKind::TailnetDerp, "shanghai")
        .expect("1 Hz host stats");
    assert_eq!(stats.capture_fps, 60);
    assert_eq!(stats.encode_p50_us, 2_004);
    assert_eq!(stats.encode_p95_us, 2_009);
    assert_eq!(stats.cpu_pct, 17);
    assert_eq!(stats.path, PathKind::TailnetDerp as i32);
    assert_eq!(stats.derp_region, "shanghai");
    assert_eq!(stats.send_kbps, 4_800);
    assert!(host
        .tick(start + 1_500_000, 17, PathKind::Lan, "")
        .is_none());
}
