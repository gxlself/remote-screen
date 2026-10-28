import CoreGraphics
import Foundation

public struct PointerEdgeEscape {
    public static let threshold: CGFloat = 160
    public static let window: TimeInterval = 0.6

    private var startedAt: TimeInterval?
    private var accumulated: CGFloat = 0

    public init() {
        startedAt = nil
    }

    public mutating func feed(atBottomEdge: Bool, viewDeltaY: CGFloat, now: TimeInterval) -> Bool {
        guard atBottomEdge else {
            reset()
            return false
        }

        let delta = viewDeltaY.isFinite ? viewDeltaY : 0
        guard delta >= 0 else {
            reset()
            return false
        }

        let time = now.isFinite ? now : 0
        if let start = startedAt, time - start > Self.window {
            startedAt = time
            accumulated = 0
        } else if startedAt == nil, delta > 0 {
            startedAt = time
        }

        accumulated += delta
        if accumulated >= Self.threshold {
            reset()
            return true
        }
        return false
    }

    public mutating func reset() {
        startedAt = nil
        accumulated = 0
    }
}
