//! Wall clock helpers and NTP-style offset estimation over Ping/Pong.
//!
//! Frames carry the host's capture time. To turn that into "how long ago" on
//! the client, the client estimates `offset = host_clock - client_clock` from
//! Ping/Pong round trips and keeps the sample with the lowest RTT, whose
//! asymmetry error is smallest (at most RTT/2).

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall clock in unix-epoch microseconds.
pub fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    pub rtt_us: u64,
    /// host − client, µs.
    pub offset_us: i64,
}

pub struct ClockSync {
    samples: VecDeque<ClockSample>,
    window: usize,
}

impl ClockSync {
    /// Keeps the last `window` samples.
    pub fn new(window: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            window: window.max(1),
        }
    }

    /// `t1`: client send time, `t2`: host receive/reply time, `t4`: client receive time.
    pub fn add(&mut self, t1: u64, t2: u64, t4: u64) -> ClockSample {
        let rtt_us = t4.saturating_sub(t1);
        let midpoint = (t1 as i128 + t4 as i128) / 2;
        let offset_us = (t2 as i128 - midpoint) as i64;
        let s = ClockSample { rtt_us, offset_us };
        self.samples.push_back(s);
        while self.samples.len() > self.window {
            self.samples.pop_front();
        }
        s
    }

    /// Best estimate: the sample with the lowest RTT in the window.
    pub fn best(&self) -> Option<ClockSample> {
        self.samples.iter().min_by_key(|s| s.rtt_us).copied()
    }

    pub fn latest(&self) -> Option<ClockSample> {
        self.samples.back().copied()
    }

    /// Converts a host timestamp to the client's clock.
    pub fn host_to_local(&self, host_us: u64) -> Option<i64> {
        self.best().map(|s| host_us as i64 - s.offset_us)
    }
}

/// Ping cadence for one client session.
///
/// The first probe is due immediately. Probes are 200 ms apart for the first
/// five seconds, then one second apart. `now_us` values are supplied by the
/// caller; this type never reads a system clock.
#[derive(Debug, Clone, Default)]
pub struct PingSchedule {
    session_start_us: Option<u64>,
    next_due_us: Option<u64>,
    fast_remaining: u8,
}

impl PingSchedule {
    /// Starts lazily: the first `next_due` is "now" and the first `on_sent`
    /// records the session start.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a session at a known time. The first probe is due at `start_us`.
    pub fn starting_at(start_us: u64) -> Self {
        Self {
            session_start_us: Some(start_us),
            next_due_us: Some(start_us),
            fast_remaining: 0,
        }
    }

    /// Session startup: the first eight probes are 60 ms apart, followed by
    /// the regular §9 cadence (200 ms until five seconds, then one second).
    /// The original constructors retain their established A10 behavior.
    pub fn fast_starting_at(start_us: u64) -> Self {
        Self {
            fast_remaining: 8,
            ..Self::starting_at(start_us)
        }
    }

    /// Time at which the next Ping should be sent. A past deadline is returned
    /// unchanged so a late caller can send immediately and resynchronize.
    pub fn next_due(&self, now_us: u64) -> u64 {
        self.next_due_us.unwrap_or(now_us)
    }

    /// Records that a Ping was sent at `now_us` and schedules the next one.
    pub fn on_sent(&mut self, now_us: u64) {
        let start_us = match self.session_start_us {
            Some(start_us) => start_us,
            None => {
                self.session_start_us = Some(now_us);
                now_us
            }
        };
        let gap_us = if self.fast_remaining > 1 {
            60_000
        } else if now_us.saturating_sub(start_us) < 5_000_000 {
            200_000
        } else {
            1_000_000
        };
        self.fast_remaining = self.fast_remaining.saturating_sub(1);
        self.next_due_us = Some(now_us.saturating_add(gap_us));
    }
}

/// Median RTT of the most recent `n` samples, or `None` when there is no
/// sample. For an even count this returns the mean of the two middle values
/// (rounded down, as the result is in whole microseconds).
pub fn rtt_median(samples: &[ClockSample], n: usize) -> Option<u64> {
    let count = n.min(samples.len());
    if count == 0 {
        return None;
    }

    let mut rtts: Vec<u64> = samples[samples.len() - count..]
        .iter()
        .map(|sample| sample.rtt_us)
        .collect();
    rtts.sort_unstable();
    let middle = count / 2;
    if count % 2 == 1 {
        Some(rtts[middle])
    } else {
        let sum = u128::from(rtts[middle - 1]) + u128::from(rtts[middle]);
        Some((sum / 2) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_start_has_eight_probes_then_regular_cadence() {
        let mut schedule = PingSchedule::fast_starting_at(1_000_000);
        let mut now = 1_000_000;
        for index in 0..8 {
            assert_eq!(schedule.next_due(now), 1_000_000 + index * 60_000);
            now = schedule.next_due(now);
            schedule.on_sent(now);
        }
        assert_eq!(schedule.next_due(now), now + 200_000);
        schedule.on_sent(6_000_000);
        assert_eq!(schedule.next_due(6_000_000), 7_000_000);
    }

    #[test]
    fn symmetric_path_recovers_exact_offset() {
        // Host clock runs 5 ms ahead; 1 ms each way.
        let mut c = ClockSync::new(8);
        let t1 = 100_000;
        let t2 = t1 + 1_000 + 5_000;
        let t4 = t1 + 2_000;
        let s = c.add(t1, t2, t4);
        assert_eq!(
            s,
            ClockSample {
                rtt_us: 2_000,
                offset_us: 5_000
            }
        );
        assert_eq!(c.host_to_local(1_005_000), Some(1_000_000));
    }

    #[test]
    fn negative_offset_and_min_rtt_selection() {
        let mut c = ClockSync::new(3);
        // Host 3 ms behind. Noisy sample with an asymmetric 9 ms path first.
        c.add(10_000, 10_000 + 9_000 - 3_000, 10_000 + 10_000);
        c.add(50_000, 50_000 + 500 - 3_000, 50_000 + 1_000);
        assert_eq!(c.best().unwrap().offset_us, -3_000);
        assert_eq!(c.latest().unwrap().rtt_us, 1_000);
    }

    #[test]
    fn window_drops_old_samples() {
        let mut c = ClockSync::new(2);
        c.add(0, 0, 10);
        c.add(100, 100, 300);
        c.add(1_000, 1_000, 1_400);
        assert_eq!(c.best().unwrap().rtt_us, 200);
    }

    #[test]
    fn unix_micros_is_plausible() {
        assert!(unix_micros() > 1_700_000_000_000_000);
    }
}
