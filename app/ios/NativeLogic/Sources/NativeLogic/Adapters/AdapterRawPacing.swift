import Foundation

/// Controls pull coalescing and the minimum interval between raw-frame pulls.
struct AdapterRawPacing {
    private let minInterval: TimeInterval
    private var latestSeq: UInt64 = 0
    private var queued = false
    private var pulling = false
    private var lastBegin: TimeInterval?

    init(minInterval: TimeInterval = 1.0 / 60) {
        self.minInterval = max(0, minInterval)
    }

    /// Records the newest ready sequence and schedules at most one pull.
    mutating func ready(seq: UInt64, now: TimeInterval) -> TimeInterval? {
        latestSeq = max(latestSeq, seq)
        guard !queued, !pulling else { return nil }
        queued = true
        return nextDelay(now: now)
    }

    /// Starts the scheduled pull and returns the newest sequence known so far.
    mutating func begin(now: TimeInterval) -> UInt64 {
        queued = false
        pulling = true
        lastBegin = now
        return latestSeq
    }

    /// Completes a pull. A successful pull that is behind the newest ready
    /// sequence schedules one more coalesced pull.
    mutating func end(pulledSeq: UInt64, now: TimeInterval) -> TimeInterval? {
        pulling = false
        guard pulledSeq != 0, latestSeq > pulledSeq else { return nil }
        queued = true
        return nextDelay(now: now)
    }

    mutating func reset() {
        latestSeq = 0
        queued = false
        pulling = false
        lastBegin = nil
    }

    private func nextDelay(now: TimeInterval) -> TimeInterval {
        guard let lastBegin else { return 0 }
        return max(0, lastBegin + minInterval - now)
    }
}
