import Foundation

/// Computes the amount of time the app may hold an iOS background task.
///
/// The extra one second of headroom gives the Rust session layer time to send
/// its goodbye message before iOS expires the task.
public enum LifecycleBackgroundBudget {
    public static let maximumHold: TimeInterval = 32
    public static let expirationHeadroom: TimeInterval = 1

    public static func holdUntil(remaining: TimeInterval) -> TimeInterval {
        guard remaining.isFinite else { return maximumHold }
        return min(maximumHold, max(0, remaining - expirationHeadroom))
    }
}
