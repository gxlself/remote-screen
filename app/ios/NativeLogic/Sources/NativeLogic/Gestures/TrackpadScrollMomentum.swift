import CoreGraphics
import Foundation

public struct TrackpadScrollMomentum {
    public private(set) var velocity: CGPoint = .zero
    public private(set) var isActive = false
    private var elapsed: TimeInterval = 0
    private var firstFrame = false

    public init() {}

    public mutating func start(velocity: CGPoint) {
        self.velocity = velocity
        elapsed = 0
        isActive = velocity.x.isFinite && velocity.y.isFinite && hypot(velocity.x, velocity.y) > 100
        firstFrame = isActive
    }

    /// phase describes the momentum phase; the gesture's phase is always 0.
    public mutating func step(dt: TimeInterval) -> (dx: CGFloat, dy: CGFloat, phase: UInt8, finished: Bool) {
        guard isActive else { return (0, 0, 3, true) }
        guard dt.isFinite, dt > 0 else { return (0, 0, 0, false) }
        let deltaTime = min(dt, max(0, 2 - elapsed))
        let dx = velocity.x * CGFloat(deltaTime) * 2
        let dy = velocity.y * CGFloat(deltaTime) * 2
        let decay = CGFloat(pow(0.95, deltaTime * 60))
        velocity = CGPoint(x: velocity.x * decay, y: velocity.y * decay)
        elapsed += deltaTime
        if hypot(velocity.x, velocity.y) < 20 || elapsed >= 2 - 1e-9 {
            isActive = false
            return (dx, dy, 3, true)
        }
        let phase: UInt8 = firstFrame ? 1 : 2
        firstFrame = false
        return (dx, dy, phase, false)
    }

    @discardableResult
    public mutating func stop() -> Bool {
        let wasActive = isActive
        isActive = false
        velocity = .zero
        return wasActive
    }
}
