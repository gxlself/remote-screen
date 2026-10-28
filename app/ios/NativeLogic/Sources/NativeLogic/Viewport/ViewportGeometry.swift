import CoreGraphics

/// Pure geometry calculations used by the viewport-aware video path.
///
/// All rectangles that describe a source area use normalized display
/// coordinates with a top-left origin.
public enum ViewportGeometry {
    private static let fullRect = CGRect(x: 0, y: 0, width: 1, height: 1)
    private static let fullTolerance: CGFloat = 0.0001
    private static let cursorTolerance: CGFloat = 0.000001

    /// Returns the part of the remote display that is visible in the view.
    ///
    /// The view bounds are mapped back through the viewport transform without
    /// first clamping the view points. This keeps the letterbox area in the
    /// calculation, then clamps the resulting normalized rectangle to the
    /// display.
    public static func visibleRect(viewport: SessionViewport, viewSize: CGSize,
                                  keyboardInset: CGFloat) -> CGRect {
        guard isFinitePositive(viewSize.width), isFinitePositive(viewSize.height),
              isUsableContentRect(viewport.contentRect),
              viewport.zoom.isFinite, viewport.zoom > 0,
              viewport.offset.x.isFinite, viewport.offset.y.isFinite,
              keyboardInset.isFinite else {
            return fullRect
        }

        let safeKeyboardInset = min(max(keyboardInset, 0), viewSize.height)
        let availableHeight = viewSize.height - safeKeyboardInset
        let widthScale = viewport.zoom * viewport.contentRect.width
        let heightScale = viewport.zoom * viewport.contentRect.height
        guard widthScale.isFinite, widthScale > 0,
              heightScale.isFinite, heightScale > 0 else {
            return fullRect
        }

        let left = viewport.offset.x + (0 - viewport.contentRect.minX) / widthScale
        let right = viewport.offset.x
            + (viewSize.width - viewport.contentRect.minX) / widthScale
        let top = viewport.offset.y + (0 - viewport.contentRect.minY) / heightScale
        let bottom = viewport.offset.y
            + (availableHeight - viewport.contentRect.minY) / heightScale
        guard left.isFinite, right.isFinite, top.isFinite, bottom.isFinite else {
            return fullRect
        }

        let minX = clampUnit(left)
        let maxX = clampUnit(right)
        let minY = clampUnit(top)
        let maxY = clampUnit(bottom)
        return CGRect(x: minX, y: minY,
                      width: max(0, maxX - minX), height: max(0, maxY - minY))
    }

    /// Converts the normalized visible rectangle to physical view pixels.
    public static func viewPixels(visible: CGRect, viewport: SessionViewport,
                                  screenScale: CGFloat) -> (width: UInt32, height: UInt32) {
        guard isFiniteRect(visible),
              visible.width >= 0, visible.height >= 0,
              isUsableContentRect(viewport.contentRect),
              viewport.zoom.isFinite, viewport.zoom > 0,
              screenScale.isFinite, screenScale >= 0 else {
            return (0, 0)
        }

        let width = visible.width * viewport.zoom * viewport.contentRect.width * screenScale
        let height = visible.height * viewport.zoom * viewport.contentRect.height * screenScale
        return (pixelCount(width), pixelCount(height))
    }

    /// Sanitizes a protocol source rectangle into normalized display space.
    public static func source(x: Float, y: Float, w: Float, h: Float) -> CGRect {
        guard x.isFinite, y.isFinite, w.isFinite, h.isFinite,
              w > 0, h > 0 else {
            return fullRect
        }

        let sourceX = CGFloat(x)
        let sourceY = CGFloat(y)
        let sourceWidth = CGFloat(w)
        let sourceHeight = CGFloat(h)
        guard sourceX.isFinite, sourceY.isFinite,
              sourceWidth.isFinite, sourceHeight.isFinite else {
            return fullRect
        }

        let minX = clampUnit(sourceX)
        let maxX = clampUnit(sourceX + sourceWidth)
        let minY = clampUnit(sourceY)
        let maxY = clampUnit(sourceY + sourceHeight)
        return CGRect(x: minX, y: minY,
                      width: max(0, maxX - minX), height: max(0, maxY - minY))
    }

    /// Returns whether a source rectangle is effectively the complete display.
    public static func isFull(_ src: CGRect) -> Bool {
        guard isFiniteRect(src) else { return false }
        return abs(src.origin.x) <= fullTolerance
            && abs(src.origin.y) <= fullTolerance
            && abs(src.width - 1) <= fullTolerance
            && abs(src.height - 1) <= fullTolerance
    }

    /// Places a cropped decoded image in the corresponding part of the
    /// content layer.
    public static func cropFrame(contentRect: CGRect, src: CGRect) -> CGRect {
        guard isNonNegativeRect(contentRect), isUsableSource(src) else {
            return .zero
        }

        let x = contentRect.minX + src.minX * contentRect.width
        let y = contentRect.minY + src.minY * contentRect.height
        let width = src.width * contentRect.width
        let height = src.height * contentRect.height
        guard x.isFinite, y.isFinite, width.isFinite, height.isFinite else {
            return .zero
        }
        return CGRect(x: x, y: y, width: width, height: height)
    }

    /// Computes the logical full-display size represented by a cropped frame.
    public static func logicalVideoSize(decoded: CGSize, src: CGRect) -> CGSize {
        guard decoded.width.isFinite, decoded.height.isFinite,
              isUsableSource(src), !isFull(src) else { return decoded }
        return CGSize(width: (decoded.width / src.width).rounded(),
                      height: (decoded.height / src.height).rounded())
    }

    /// Maps a full-display cursor into the normalized coordinates of a crop.
    public static func cursorInCrop(_ cursor: CGPoint, src: CGRect) -> CGPoint? {
        guard cursor.x.isFinite, cursor.y.isFinite, isFiniteRect(src) else {
            return nil
        }
        if isFull(src) { return cursor }
        guard isUsableSource(src) else { return nil }

        guard cursor.x >= src.minX - cursorTolerance,
              cursor.x <= src.maxX + cursorTolerance,
              cursor.y >= src.minY - cursorTolerance,
              cursor.y <= src.maxY + cursorTolerance else {
            return nil
        }

        let x = min(max(cursor.x, src.minX), src.maxX)
        let y = min(max(cursor.y, src.minY), src.maxY)
        return CGPoint(x: (x - src.minX) / src.width,
                       y: (y - src.minY) / src.height)
    }

    private static func isFiniteRect(_ rect: CGRect) -> Bool {
        rect.minX.isFinite && rect.minY.isFinite
            && rect.width.isFinite && rect.height.isFinite
            && rect.maxX.isFinite && rect.maxY.isFinite
    }

    private static func isUsableContentRect(_ rect: CGRect) -> Bool {
        isFiniteRect(rect) && rect.width > 0 && rect.height > 0
    }

    private static func isNonNegativeRect(_ rect: CGRect) -> Bool {
        isFiniteRect(rect) && rect.width >= 0 && rect.height >= 0
    }

    private static func isUsableSource(_ src: CGRect) -> Bool {
        isFiniteRect(src) && src.width > 0 && src.height > 0
            && src.minX >= 0 && src.minY >= 0
            && src.maxX <= 1 && src.maxY <= 1
    }

    private static func clampUnit(_ value: CGFloat) -> CGFloat {
        if value.isNaN { return 0 }
        if value == .infinity { return 1 }
        if value == -.infinity { return 0 }
        return min(max(value, 0), 1)
    }

    private static func isFinitePositive(_ value: CGFloat) -> Bool {
        value.isFinite && value > 0
    }

    private static func pixelCount(_ value: CGFloat) -> UInt32 {
        guard !value.isNaN, value > 0 else { return 0 }
        if value == .infinity { return UInt32.max }
        let roundedDown = value.rounded(.down)
        guard roundedDown < CGFloat(UInt32.max) else { return UInt32.max }
        return UInt32(roundedDown)
    }
}
