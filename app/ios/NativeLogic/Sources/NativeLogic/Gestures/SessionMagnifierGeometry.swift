import CoreGraphics

public enum SessionMagnifierGeometry {
    public static func frame(finger: CGPoint, viewBounds: CGRect, diameter: CGFloat,
                             offset: CGPoint = CGPoint(x: 0, y: -80)) -> CGRect {
        let diameter = max(diameter.isFinite ? diameter : 0, 1)
        let radius = diameter / 2
        let bounds = viewBounds.standardized
        var center = CGPoint(x: finger.x + offset.x, y: finger.y + offset.y)
        // If the preferred position would overlap the top edge, put the
        // magnifier below the finger. This is the iPad touch-mode behavior.
        if center.y - radius < bounds.minY {
            center.y = finger.y - offset.y
        }
        center.x = min(max(center.x, bounds.minX + radius), bounds.maxX - radius)
        center.y = min(max(center.y, bounds.minY + radius), bounds.maxY - radius)
        return CGRect(x: center.x - radius, y: center.y - radius,
                      width: diameter, height: diameter)
    }

    /// Computes the source rectangle in video-pixel coordinates. The input
    /// cursor uses normalized, top-left-origin coordinates while Core Image
    /// callers can flip the y axis when cropping.
    public static func sourceRect(cursorNormalized: CGPoint, videoSize: CGSize,
                                  zoom: CGFloat, diameterPt: CGFloat,
                                  pointsPerVideoPixel: CGFloat) -> CGRect {
        let width = max(videoSize.width.isFinite ? videoSize.width : 0, 1)
        let height = max(videoSize.height.isFinite ? videoSize.height : 0, 1)
        let ppp = max(pointsPerVideoPixel.isFinite ? pointsPerVideoPixel : 0, 0.0001)
        let safeZoom = max(1, zoom.isFinite ? zoom : 1)
        let sourceWidth = min(width, max(1, diameterPt / ppp / safeZoom))
        let sourceHeight = min(height, max(1, diameterPt / ppp / safeZoom))
        let x = min(max(cursorNormalized.x.isFinite ? cursorNormalized.x : 0.5, 0), 1)
        let y = min(max(cursorNormalized.y.isFinite ? cursorNormalized.y : 0.5, 0), 1)
        let originX = min(max(x * width - sourceWidth / 2, 0), width - sourceWidth)
        let originY = min(max(y * height - sourceHeight / 2, 0), height - sourceHeight)
        return CGRect(x: originX, y: originY, width: sourceWidth, height: sourceHeight)
    }
}
