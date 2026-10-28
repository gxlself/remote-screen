import Foundation

public struct TrackpadHostCursorArbiter {
    private var lastLocalMove: TimeInterval?

    public init() {}

    public mutating func recordLocalMove(at time: TimeInterval) {
        if time.isFinite { lastLocalMove = time }
    }

    public func shouldAcceptHost(at time: TimeInterval) -> Bool {
        guard time.isFinite else { return false }
        guard let lastLocalMove else { return true }
        return time - lastLocalMove >= 0.2
    }
}
