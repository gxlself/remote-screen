import Foundation

/// Suppresses repeated camera results while keeping different QR values
/// responsive. Callers provide the clock so the policy remains deterministic
/// in tests and does not depend on wall-clock reads.
struct QrScanThrottle {
    let windowMs: UInt64
    private var acceptedAtMs: [String: UInt64] = [:]

    init(windowMs: UInt64 = 2_000) {
        self.windowMs = windowMs
    }

    mutating func accept(_ text: String, nowMs: UInt64) -> Bool {
        // Forget expired values, including after a clock reset. Keep separate
        // timestamps so two QR codes in one frame cannot bypass each other's
        // duplicate window by alternating.
        acceptedAtMs = acceptedAtMs.filter { _, accepted in
            nowMs >= accepted && nowMs - accepted < windowMs
        }
        guard acceptedAtMs[text] == nil else { return false }
        acceptedAtMs[text] = nowMs
        return true
    }
}
