import CoreGraphics

public struct SessionViewport {
    public static let minimumZoom: CGFloat = 1
    public static let maximumZoom: CGFloat = 4

    public var contentRect: CGRect
    private var storedZoom: CGFloat
    private var storedOffset: CGPoint

    public var zoom: CGFloat {
        get { storedZoom }
        set {
            storedZoom = newValue.isFinite ? min(max(Self.minimumZoom, newValue), Self.maximumZoom) : Self.minimumZoom
            offset = storedOffset
        }
    }

    public var offset: CGPoint {
        get { storedOffset }
        set {
            let limit = 1 - 1 / zoom
            storedOffset = CGPoint(x: Self.clamp(newValue.x, upper: limit),
                                   y: Self.clamp(newValue.y, upper: limit))
        }
    }

    public init(contentRect: CGRect = .zero, zoom: CGFloat = 1, offset: CGPoint = .zero) {
        self.contentRect = contentRect
        storedZoom = zoom.isFinite ? min(max(Self.minimumZoom, zoom), Self.maximumZoom) : Self.minimumZoom
        storedOffset = .zero
        self.offset = offset
    }

    /// The part of the remote image currently visible in normalized coordinates.
    /// At 1x this is the complete image; at 2x it is a half-sized rectangle.
    public var visibleRect: CGRect {
        CGRect(x: offset.x, y: offset.y, width: 1 / zoom, height: 1 / zoom)
    }

    /// Returns a viewport scaled around a point in normalized image coordinates.
    /// The point remains under the same view location after the scale.
    public func zoomed(by factor: CGFloat, anchor: CGPoint) -> SessionViewport {
        guard factor.isFinite else { return self }
        let oldZoom = zoom
        let newZoom = min(max(Self.minimumZoom, oldZoom * max(0, factor)), Self.maximumZoom)
        guard newZoom != oldZoom else { return self }
        let safeAnchor = CGPoint(x: Self.clamp(anchor.x), y: Self.clamp(anchor.y))
        let scale = oldZoom / newZoom
        let nextOffset = CGPoint(
            x: safeAnchor.x - (safeAnchor.x - offset.x) * scale,
            y: safeAnchor.y - (safeAnchor.y - offset.y) * scale
        )
        return SessionViewport(contentRect: contentRect, zoom: newZoom, offset: nextOffset)
    }

    /// Returns a viewport translated by a normalized image-space delta.
    public func panned(by delta: CGPoint) -> SessionViewport {
        guard delta.x.isFinite, delta.y.isFinite else { return self }
        return SessionViewport(contentRect: contentRect, zoom: zoom,
                               offset: CGPoint(x: offset.x + delta.x, y: offset.y + delta.y))
    }

    /// Mutating convenience forms for callers that keep a viewport in state.
    public mutating func zoom(by factor: CGFloat, anchor: CGPoint) {
        self = zoomed(by: factor, anchor: anchor)
    }

    public mutating func pan(by delta: CGPoint) {
        self = panned(by: delta)
    }

    public func normalized(fromView point: CGPoint) -> CGPoint {
        guard hasContent else { return offset }
        return CGPoint(x: offset.x + Self.clamp((point.x - contentRect.minX) / contentRect.width) / zoom,
                       y: offset.y + Self.clamp((point.y - contentRect.minY) / contentRect.height) / zoom)
    }

    public func viewPoint(fromNormalized point: CGPoint) -> CGPoint {
        CGPoint(x: contentRect.minX + (point.x - offset.x) * zoom * contentRect.width,
                y: contentRect.minY + (point.y - offset.y) * zoom * contentRect.height)
    }

    public func normalizedDelta(fromViewDelta delta: CGPoint) -> CGPoint {
        guard hasContent, delta.x.isFinite, delta.y.isFinite else { return .zero }
        return CGPoint(x: delta.x / (contentRect.width * zoom),
                       y: delta.y / (contentRect.height * zoom))
    }

    private var hasContent: Bool {
        contentRect.width.isFinite && contentRect.height.isFinite &&
            contentRect.width > 0 && contentRect.height > 0
    }

    private static func clamp(_ value: CGFloat, upper: CGFloat = 1) -> CGFloat {
        value.isFinite ? min(max(value, 0), upper) : 0
    }
}
