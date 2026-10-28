import CoreGraphics

public enum TrackpadAccel {
    public static func gain(speed: CGFloat) -> CGFloat {
        guard !speed.isNaN else { return 1 }
        return 1 + min(max(speed, 0) / 900, 2)
    }
}

public struct TrackpadCursorModel {
    public private(set) var position = CGPoint(x: 0.5, y: 0.5)

    public init(position: CGPoint = CGPoint(x: 0.5, y: 0.5)) {
        move(to: position)
    }

    public mutating func move(to point: CGPoint) {
        guard point.x.isFinite, point.y.isFinite else { return }
        position = CGPoint(x: min(max(point.x, 0), 1), y: min(max(point.y, 0), 1))
    }

    public mutating func apply(viewDelta: CGPoint, contentSize: CGSize, gain: CGFloat) {
        apply(viewDelta: viewDelta,
              viewport: SessionViewport(contentRect: CGRect(origin: .zero, size: contentSize)), gain: gain)
    }

    public mutating func apply(viewDelta: CGPoint, viewport: SessionViewport, gain: CGFloat) {
        guard gain.isFinite, gain >= 0 else { return }
        let delta = viewport.normalizedDelta(fromViewDelta: viewDelta)
        move(to: CGPoint(x: position.x + delta.x * gain, y: position.y + delta.y * gain))
    }
}
