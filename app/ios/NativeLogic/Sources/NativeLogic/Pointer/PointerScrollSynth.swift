import CoreGraphics
import Foundation

public struct PointerScrollSynth {
    public struct Output: Equatable {
        public let dx: CGFloat
        public let dy: CGFloat
        public let phase: UInt8

        public init(dx: CGFloat, dy: CGFloat, phase: UInt8) {
            self.dx = dx
            self.dy = dy
            self.phase = phase
        }
    }

    public static let idleEnd: TimeInterval = 0.12
    public static let pointsPerUnit: CGFloat = 10

    private var lastFeed: TimeInterval?

    public init() {
        lastFeed = nil
    }

    public mutating func feed(x: CGFloat, y: CGFloat, now: TimeInterval) -> Output? {
        guard x.isFinite, y.isFinite, x != 0 || y != 0 else { return nil }

        let phase: UInt8 = lastFeed == nil ? 1 : 2
        lastFeed = now.isFinite ? now : 0
        return Output(dx: x * Self.pointsPerUnit,
                      dy: y * Self.pointsPerUnit,
                      phase: phase)
    }

    public mutating func idle(now: TimeInterval) -> Output? {
        guard let lastFeed else { return nil }
        let time = now.isFinite ? now : 0
        guard time - lastFeed >= Self.idleEnd else { return nil }
        self.lastFeed = nil
        return Output(dx: 0, dy: 0, phase: 3)
    }

    public mutating func reset() -> Output? {
        guard lastFeed != nil else { return nil }
        lastFeed = nil
        return Output(dx: 0, dy: 0, phase: 3)
    }
}
