import Foundation

public struct SessionDecodeStatsSnapshot: Equatable {
    public let fps: Int
    public let decodeP50Ms: Double
    public let decodeP95Ms: Double
    public let latencyP50Ms: Double
    public let latestDecodeUs: UInt32?
    public let latestLatencyUs: UInt32?

    public init(fps: Int, decodeP50Ms: Double, decodeP95Ms: Double,
                latencyP50Ms: Double, latestDecodeUs: UInt32?, latestLatencyUs: UInt32?) {
        self.fps = fps
        self.decodeP50Ms = decodeP50Ms
        self.decodeP95Ms = decodeP95Ms
        self.latencyP50Ms = latencyP50Ms
        self.latestDecodeUs = latestDecodeUs
        self.latestLatencyUs = latestLatencyUs
    }
}

/// Clock-injected decode and presentation measurements. It retains only the
/// 120 samples required by the session HUD and a one-second presentation
/// window for fps.
public final class SessionDecodeStats {
    public let sampleLimit: Int
    private var decodeSamples: [Double] = []
    private var latencySamples: [Double] = []
    private var presentationTimes: [TimeInterval] = []
    private(set) public var latestDecodeUs: UInt32?
    private(set) public var latestLatencyUs: UInt32?

    public init(sampleLimit: Int = 120) {
        self.sampleLimit = max(1, sampleLimit)
    }

    public var hasDecodeSamples: Bool { !decodeSamples.isEmpty }

    public func recordDecode(durationMs: Double, at _: TimeInterval) {
        guard durationMs.isFinite, durationMs >= 0 else { return }
        append(durationMs, to: &decodeSamples)
        latestDecodeUs = Self.microseconds(durationMs)
    }

    public func recordLatency(ms: Double) {
        guard ms.isFinite, ms >= 0 else { return }
        append(ms, to: &latencySamples)
        latestLatencyUs = Self.microseconds(ms)
    }

    public func recordPresented(at now: TimeInterval) {
        guard now.isFinite else { return }
        presentationTimes.append(now)
        trimPresentationTimes(through: now)
    }

    public func recordFrame(decodeMs: Double, latencyMs: Double?, presentedAt: TimeInterval?) {
        let now = presentedAt ?? 0
        recordDecode(durationMs: decodeMs, at: now)
        // Reports describe the latest frame, even if older frames supplied
        // valid clock samples that remain in the independent latency window.
        latestLatencyUs = nil
        if let latencyMs { recordLatency(ms: latencyMs) }
        if let presentedAt { recordPresented(at: presentedAt) }
    }

    public func snapshot(at now: TimeInterval) -> SessionDecodeStatsSnapshot {
        trimPresentationTimes(through: now)
        let decode = decodeSamples.sorted()
        let latency = latencySamples.sorted()
        return SessionDecodeStatsSnapshot(
            fps: presentationTimes.count,
            decodeP50Ms: percentile(decode, 0.50) ?? -1,
            decodeP95Ms: percentile(decode, 0.95) ?? -1,
            latencyP50Ms: percentile(latency, 0.50) ?? -1,
            latestDecodeUs: latestDecodeUs,
            latestLatencyUs: latestLatencyUs)
    }

    public func reset() {
        decodeSamples.removeAll(keepingCapacity: true)
        latencySamples.removeAll(keepingCapacity: true)
        presentationTimes.removeAll(keepingCapacity: true)
        latestDecodeUs = nil
        latestLatencyUs = nil
    }

    private func append(_ value: Double, to samples: inout [Double]) {
        samples.append(value)
        if samples.count > sampleLimit {
            samples.removeFirst(samples.count - sampleLimit)
        }
    }

    private func trimPresentationTimes(through now: TimeInterval) {
        let cutoff = now - 1
        presentationTimes.removeAll { $0 <= cutoff }
    }

    private func percentile(_ sorted: [Double], _ p: Double) -> Double? {
        guard !sorted.isEmpty else { return nil }
        // Nearest-rank percentile: ceil(p * n), one-based. The task requires
        // 1...120 to give p50=60 and p95=114; M0 used rounded array indices.
        let rank = max(1, Int(ceil(p * Double(sorted.count))))
        return sorted[min(rank - 1, sorted.count - 1)]
    }

    private static func microseconds(_ milliseconds: Double) -> UInt32 {
        let value = max(0, min(milliseconds * 1_000, Double(UInt32.max)))
        return UInt32(value.rounded())
    }
}
