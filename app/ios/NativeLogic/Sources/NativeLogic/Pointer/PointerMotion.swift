import CoreGraphics
import Foundation

public struct PointerMotion {
    public static let slowSpeed: CGFloat = 150
    public static let fastSpeed: CGFloat = 1500
    public static let maxGain: CGFloat = 3
    public static let idleReset: TimeInterval = 0.1

    private var lastTime: TimeInterval?
    private var smoothedSpeed: CGFloat?

    public init() {
        lastTime = nil
        smoothedSpeed = nil
    }

    public static func gain(speed: CGFloat) -> CGFloat {
        guard !speed.isNaN, speed >= 0 else { return 1 }
        if speed <= slowSpeed { return 1 }
        if speed >= fastSpeed { return maxGain }
        let fraction = (speed - slowSpeed) / (fastSpeed - slowSpeed)
        return 1 + fraction * (maxGain - 1)
    }

    public mutating func viewDelta(raw: CGPoint, now: TimeInterval) -> CGPoint {
        guard raw.x.isFinite, raw.y.isFinite else { return .zero }

        let defaultDelta = 1 / 120.0
        var deltaTime = defaultDelta
        var resetHistory = true
        if let lastTime, now.isFinite, lastTime.isFinite {
            let elapsed = now - lastTime
            if elapsed.isFinite, elapsed > 0, elapsed <= Self.idleReset {
                deltaTime = elapsed
                resetHistory = false
            }
        }

        let instantaneousSpeed = hypot(raw.x, raw.y) / CGFloat(deltaTime)
        let speed: CGFloat
        if resetHistory {
            speed = instantaneousSpeed
        } else if let previousSpeed = smoothedSpeed {
            speed = 0.5 * previousSpeed + 0.5 * instantaneousSpeed
        } else {
            speed = instantaneousSpeed
        }

        smoothedSpeed = speed
        lastTime = now.isFinite ? now : nil
        let multiplier = Self.gain(speed: speed)
        return CGPoint(x: raw.x * multiplier, y: raw.y * multiplier)
    }

    public mutating func reset() {
        lastTime = nil
        smoothedSpeed = nil
    }
}
