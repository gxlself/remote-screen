import CoreGraphics

/// Pure viewport calculations shared by the UIKit gesture controllers and
/// NativeLogic tests. Coordinates are normalized to the remote video.
public enum SessionZoomMath {
    public static let defaultMaximumZoom: CGFloat = 4
    public static let defaultFollowMargin: CGFloat = 0.1

    public static func clampOffset(_ offset: CGPoint, zoom: CGFloat) -> CGPoint {
        let safeZoom = zoom.isFinite ? min(max(1, zoom), defaultMaximumZoom) : 1
        let limit = max(0, 1 - 1 / safeZoom)
        return CGPoint(x: clamp(offset.x, upper: limit), y: clamp(offset.y, upper: limit))
    }

    public static func clampOffset(offset: CGPoint, zoom: CGFloat) -> CGPoint {
        clampOffset(offset, zoom: zoom)
    }

    /// Resize the aspect-fit content while preserving the normalized visible
    /// center and zoom. Visible fractions are independent of view points.
    /// Transient zero-sized layouts must not discard the previous viewport.
    public static func relayout(viewport: SessionViewport, oldSize: CGSize,
                                newSize: CGSize) -> SessionViewport {
        guard newSize.width.isFinite, newSize.height.isFinite,
              newSize.width > 0, newSize.height > 0 else { return viewport }
        let previous = viewport.visibleRect
        let center = CGPoint(x: previous.midX, y: previous.midY)
        let sourceSize = viewport.contentRect.width > 0 && viewport.contentRect.height > 0
            ? viewport.contentRect.size : oldSize
        let contentRect = SessionContentRect.fit(video: sourceSize, in: newSize)
        let halfExtent = 0.5 / viewport.zoom
        let offset = clampOffset(CGPoint(x: center.x - halfExtent, y: center.y - halfExtent),
                                 zoom: viewport.zoom)
        return SessionViewport(contentRect: contentRect, zoom: viewport.zoom, offset: offset)
    }

    /// Keeps a cursor away from the edge of a zoomed viewport. The returned
    /// value is a viewport so its content rectangle and zoom are preserved.
    public static func followCursor(cursor: CGPoint, viewport: SessionViewport,
                                    margin: CGFloat = defaultFollowMargin) -> SessionViewport {
        let visible = viewport.visibleRect
        let safeMargin = min(max(margin.isFinite ? margin : defaultFollowMargin, 0), 0.49)
        let safeCursor = CGPoint(x: clamp(cursor.x), y: clamp(cursor.y))
        let xMargin = visible.width * safeMargin
        let yMargin = visible.height * safeMargin
        var next = viewport.offset
        let left = visible.minX + xMargin
        let right = visible.maxX - xMargin
        let top = visible.minY + yMargin
        let bottom = visible.maxY - yMargin
        if safeCursor.x < left { next.x -= left - safeCursor.x }
        if safeCursor.x > right { next.x += safeCursor.x - right }
        if safeCursor.y < top { next.y -= top - safeCursor.y }
        if safeCursor.y > bottom { next.y += safeCursor.y - bottom }
        return SessionViewport(contentRect: viewport.contentRect, zoom: viewport.zoom, offset: next)
    }

    /// Convenience form matching callers that keep the viewport at its
    /// default value and only need a pure, clamped result.
    public static func followCursor(cursor: CGPoint, margin: CGFloat,
                                    viewport: SessionViewport = SessionViewport()) -> SessionViewport {
        followCursor(cursor: cursor, viewport: viewport, margin: margin)
    }

    /// Offset-only form useful when the caller already owns a viewport.
    public static func followCursor(cursor: CGPoint, offset: CGPoint, zoom: CGFloat,
                                    margin: CGFloat = defaultFollowMargin) -> CGPoint {
        let viewport = SessionViewport(zoom: zoom, offset: offset)
        return followCursor(cursor: cursor, viewport: viewport, margin: margin).offset
    }

    /// A target used when the software keyboard is visible. The center is the
    /// center of the area above the keyboard, expressed in view points.
    public struct KeyboardTarget: Equatable {
        public let zoom: CGFloat
        public let offset: CGPoint

        public init(zoom: CGFloat, offset: CGPoint) {
            self.zoom = zoom
            self.offset = offset
        }

        public var viewport: SessionViewport {
            SessionViewport(zoom: zoom, offset: offset)
        }
    }

    public static func keyboardTarget(cursor: CGPoint, inset: CGFloat, viewSize: CGSize,
                                      minZoom: CGFloat) -> KeyboardTarget {
        let width = max(viewSize.width.isFinite ? viewSize.width : 0, 1)
        let height = max(viewSize.height.isFinite ? viewSize.height : 0, 1)
        let safeInset = min(max(inset.isFinite ? inset : 0, 0), height - 1)
        let zoom = min(max(1, minZoom.isFinite ? minZoom : 1), defaultMaximumZoom)
        let availableHeight = height - safeInset
        let center = CGPoint(x: width / 2, y: availableHeight / 2)
        let safeCursor = CGPoint(x: clamp(cursor.x), y: clamp(cursor.y))
        let nextOffset = CGPoint(
            x: safeCursor.x - center.x / (width * zoom),
            y: safeCursor.y - center.y / (height * zoom)
        )
        // Keep the mathematical target even when the cursor is near the lower
        // edge. The viewport owner applies its normal clamp when a target would
        // require showing pixels outside the remote image.
        return KeyboardTarget(zoom: zoom, offset: nextOffset)
    }

    public static func keyboardTarget(cursor: CGPoint, inset: CGFloat, viewSize: CGSize,
                                      minZoom: CGFloat, currentZoom: CGFloat) -> KeyboardTarget {
        let safeCurrentZoom = max(1, currentZoom.isFinite ? currentZoom : 1)
        let safeMinimumZoom = min(max(1, minZoom.isFinite ? minZoom : 1), defaultMaximumZoom)
        let targetZoom = safeCurrentZoom >= safeMinimumZoom
            ? min(safeCurrentZoom, defaultMaximumZoom)
            : safeMinimumZoom
        let target = keyboardTarget(cursor: cursor, inset: inset, viewSize: viewSize,
                                    minZoom: targetZoom)
        return KeyboardTarget(zoom: targetZoom, offset: target.offset)
    }

    private static func clamp(_ value: CGFloat, upper: CGFloat = 1) -> CGFloat {
        value.isFinite ? min(max(value, 0), upper) : 0
    }
}
