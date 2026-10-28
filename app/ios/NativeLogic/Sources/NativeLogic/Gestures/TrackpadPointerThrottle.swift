import CoreGraphics
import Foundation

public struct TrackpadPointerThrottle {
    public static let interval: TimeInterval = 1 / 120
    public private(set) var lastEmissionTime: TimeInterval?
    public var hasPending: Bool { pending != nil }
    private var pending: CGPoint?

    public init() {}

    public mutating func offer(_ point: CGPoint, now: TimeInterval) -> CGPoint? {
        guard point.x.isFinite, point.y.isFinite, now.isFinite else { return nil }
        pending = point
        if let lastEmissionTime, now - lastEmissionTime < Self.interval { return nil }
        return flush(now: now)
    }

    /// Extracts the trailing sample. The ABI sender schedules this at the next
    /// allowed deadline, including when the gesture ends between two samples.
    public mutating func flush(now: TimeInterval) -> CGPoint? {
        guard let pending, now.isFinite else { return nil }
        self.pending = nil
        lastEmissionTime = now
        return pending
    }
}
