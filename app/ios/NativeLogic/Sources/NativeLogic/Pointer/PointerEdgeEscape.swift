import CoreGraphics
import Foundation

/// Releases the pointer lock when the cursor is held against the bottom edge
/// and pushed down on purpose. Flicks onto the taskbar or a full-screen
/// player's controls must not escape: the click that relocks is swallowed.
public struct PointerEdgeEscape {
    public static let threshold: CGFloat = 240
    /// The push must last this long; a flick is over well before it.
    public static let minDuration: TimeInterval = 0.5
    /// A pause longer than this starts over.
    public static let maxGap: TimeInterval = 0.15

    private var startedAt: TimeInterval?
    private var lastPushAt: TimeInterval = 0
    private var accumulated: CGFloat = 0

    public init() {
        startedAt = nil
    }

    public mutating func feed(atBottomEdge: Bool, viewDelta: CGVector, now: TimeInterval) -> Bool {
        let dx = viewDelta.dx.isFinite ? viewDelta.dx : 0
        let dy = viewDelta.dy.isFinite ? viewDelta.dy : 0
        // Only a mostly downward push counts; moving along the edge starts over.
        guard atBottomEdge, dy > 0, dy >= abs(dx) else {
            reset()
            return false
        }

        let time = now.isFinite ? now : 0
        if let start = startedAt, time >= start, time - lastPushAt <= Self.maxGap {
            accumulated += dy
        } else {
            startedAt = time
            accumulated = dy
        }
        lastPushAt = time

        if let start = startedAt, time - start >= Self.minDuration, accumulated >= Self.threshold {
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
