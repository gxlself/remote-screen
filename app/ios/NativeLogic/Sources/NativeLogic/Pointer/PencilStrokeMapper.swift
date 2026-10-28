import CoreGraphics
import Foundation

public enum PencilAction: Equatable {
    case move(CGPoint)
    case down(CGPoint, clicks: UInt8)
    case up(CGPoint, clicks: UInt8)
}

public struct PencilStrokeMapper {
    public static let dragSlop: CGFloat = 4
    public static let hoverEpsilon: CGFloat = 0.5

    public private(set) var isDown = false

    private var clickCounter = TrackpadClickCounter()
    private var downViewPoint: CGPoint?
    private var downNormalizedPoint: CGPoint?
    private var lastOutputPoint: CGPoint?
    private var lastHoverViewPoint: CGPoint?
    private var clicks: UInt8 = 1
    private var crossedDragSlop = false

    public init() {}

    public mutating func hover(viewPoint: CGPoint, viewport: SessionViewport) -> [PencilAction] {
        guard !isDown else { return [] }
        if let lastHoverViewPoint,
           Self.distance(viewPoint, lastHoverViewPoint) < Self.hoverEpsilon {
            return []
        }
        lastHoverViewPoint = viewPoint
        let normalized = TouchTapMapper.normalized(viewPoint: viewPoint, viewport: viewport)
        lastOutputPoint = normalized
        return [.move(normalized)]
    }

    public mutating func hoverEnded() {
        lastHoverViewPoint = nil
    }

    public mutating func began(viewPoint: CGPoint, viewport: SessionViewport,
                               time: TimeInterval) -> [PencilAction] {
        guard !isDown else { return [] }
        let normalized = TouchTapMapper.normalized(viewPoint: viewPoint, viewport: viewport)
        let count = clickCounter.register(position: normalized, time: time)
        clicks = UInt8(count)
        isDown = true
        downViewPoint = viewPoint
        downNormalizedPoint = normalized
        lastOutputPoint = normalized
        crossedDragSlop = false
        return [.move(normalized), .down(normalized, clicks: clicks)]
    }

    public mutating func moved(viewPoint: CGPoint, viewport: SessionViewport) -> [PencilAction] {
        guard isDown, let downViewPoint else { return [] }
        if !crossedDragSlop {
            guard Self.distance(viewPoint, downViewPoint) >= Self.dragSlop else { return [] }
            crossedDragSlop = true
        }
        let normalized = TouchTapMapper.normalized(viewPoint: viewPoint, viewport: viewport)
        lastOutputPoint = normalized
        return [.move(normalized)]
    }

    public mutating func ended(viewPoint: CGPoint, viewport: SessionViewport) -> [PencilAction] {
        guard isDown, let downNormalizedPoint, let downViewPoint else { return [] }
        if !crossedDragSlop, Self.distance(viewPoint, downViewPoint) >= Self.dragSlop {
            crossedDragSlop = true
        }
        let normalized = TouchTapMapper.normalized(viewPoint: viewPoint, viewport: viewport)
        let actions: [PencilAction]
        if crossedDragSlop {
            lastOutputPoint = normalized
            actions = [.move(normalized), .up(normalized, clicks: clicks)]
        } else {
            actions = [.up(downNormalizedPoint, clicks: clicks)]
        }
        clearDownState()
        return actions
    }

    public mutating func cancelled() -> [PencilAction] {
        guard isDown else { return [] }
        let point = lastOutputPoint ?? downNormalizedPoint ?? .zero
        let actions: [PencilAction] = [.up(point, clicks: clicks)]
        clearDownState()
        return actions
    }

    public mutating func reset() -> [PencilAction] {
        let actions = cancelled()
        clickCounter = TrackpadClickCounter()
        lastHoverViewPoint = nil
        lastOutputPoint = nil
        return actions
    }

    private mutating func clearDownState() {
        isDown = false
        downViewPoint = nil
        downNormalizedPoint = nil
        lastOutputPoint = nil
        crossedDragSlop = false
    }

    private static func distance(_ lhs: CGPoint, _ rhs: CGPoint) -> CGFloat {
        let dx = lhs.x - rhs.x
        let dy = lhs.y - rhs.y
        return (dx * dx + dy * dy).squareRoot()
    }
}
