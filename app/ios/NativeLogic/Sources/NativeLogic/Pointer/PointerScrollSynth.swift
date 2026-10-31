import CoreGraphics
import Foundation

public struct PointerScrollSynth {
    public struct Output: Equatable {
        public let dx: CGFloat
        public let dy: CGFloat
        public let phase: UInt8
        /// Points per second the scroll was moving when it ended on its own
        /// (`idle`); zero otherwise. A fast one carries on as momentum.
        public let velocity: CGVector

        public init(dx: CGFloat, dy: CGFloat, phase: UInt8, velocity: CGVector = .zero) {
            self.dx = dx
            self.dy = dy
            self.phase = phase
            self.velocity = velocity
        }
    }

    /// One scroll, from its first sample to its end.
    public struct Summary: Equatable {
        public var samples = 0
        public var total = CGVector.zero
        /// The largest single sample, before the axis lock.
        public var peak: CGFloat = 0
        public var duration: TimeInterval = 0
    }

    public static let idleEnd: TimeInterval = 0.12
    /// GameController's trackpad scroll comes in about the points UIKit's
    /// scroll translation does; doubled like PointerGestureController's.
    public static let pointsPerUnit: CGFloat = 2
    /// How far a scroll goes before it picks its axis.
    public static let axisSlop: CGFloat = 2
    /// Only the samples this close to the last one set the release velocity,
    /// so fingers that slow down before stopping leave no momentum.
    public static let flickWindow: TimeInterval = 0.08
    public static let maxVelocity: CGFloat = 8000

    public private(set) var summary = Summary()
    /// A scroll keeps to one axis: a trackpad swipe up or down always drifts
    /// sideways a little, and Windows would scroll that too. Nil until the
    /// scroll has gone `axisSlop` one way or the other.
    private var vertical: Bool?
    private var pending = CGVector.zero
    private var lastFeed: TimeInterval?
    private var firstFeed: TimeInterval = 0
    private var recent: [(time: TimeInterval, delta: CGVector)] = []

    public init() {
        lastFeed = nil
    }

    public mutating func feed(x: CGFloat, y: CGFloat, now: TimeInterval) -> Output? {
        guard x.isFinite, y.isFinite, x != 0 || y != 0 else { return nil }

        let phase: UInt8 = lastFeed == nil ? 1 : 2
        let time = now.isFinite ? now : 0
        if phase == 1 {
            summary = Summary()
            firstFeed = time
            recent.removeAll()
            vertical = nil
            pending = .zero
        }
        lastFeed = time
        summary.peak = max(summary.peak, abs(x), abs(y))
        var move = CGVector(dx: x, dy: y)
        if vertical == nil {
            pending.dx += x
            pending.dy += y
            if max(abs(pending.dx), abs(pending.dy)) >= Self.axisSlop {
                vertical = abs(pending.dy) >= abs(pending.dx)
                move = pending
            } else {
                move = .zero
            }
        }
        let delta = vertical == true ? CGVector(dx: 0, dy: move.dy * Self.pointsPerUnit)
                                     : CGVector(dx: move.dx * Self.pointsPerUnit, dy: 0)
        recent.append((time, delta))
        recent.removeAll { time - $0.time > Self.flickWindow }
        summary.samples += 1
        summary.total.dx += delta.dx
        summary.total.dy += delta.dy
        summary.duration = time - firstFeed
        return Output(dx: delta.dx, dy: delta.dy, phase: phase)
    }

    public mutating func idle(now: TimeInterval) -> Output? {
        guard let lastFeed else { return nil }
        let time = now.isFinite ? now : 0
        guard time - lastFeed >= Self.idleEnd else { return nil }
        self.lastFeed = nil
        let velocity = releaseVelocity()
        recent.removeAll()
        return Output(dx: 0, dy: 0, phase: 3, velocity: velocity)
    }

    public mutating func reset() -> Output? {
        recent.removeAll()
        guard lastFeed != nil else { return nil }
        lastFeed = nil
        return Output(dx: 0, dy: 0, phase: 3)
    }

    /// The distance after the first recent sample over the time it took.
    private func releaseVelocity() -> CGVector {
        guard recent.count >= 2, let first = recent.first, let last = recent.last else { return .zero }
        let span = max(last.time - first.time, 1 / 60)
        var dx: CGFloat = 0
        var dy: CGFloat = 0
        for sample in recent.dropFirst() {
            dx += sample.delta.dx
            dy += sample.delta.dy
        }
        let velocity = CGVector(dx: dx / CGFloat(span), dy: dy / CGFloat(span))
        let speed = hypot(velocity.dx, velocity.dy)
        guard speed > Self.maxVelocity else { return velocity }
        let scale = Self.maxVelocity / speed
        return CGVector(dx: velocity.dx * scale, dy: velocity.dy * scale)
    }
}
