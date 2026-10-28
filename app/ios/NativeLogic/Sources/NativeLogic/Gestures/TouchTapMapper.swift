import CoreGraphics

public enum TouchTapMapper {
    /// Maps a view point into normalized remote-video coordinates. Letterbox
    /// bars and points outside the view are clamped to the nearest edge.
    public static func normalized(viewPoint: CGPoint, viewport: SessionViewport) -> CGPoint {
        let value = viewport.normalized(fromView: viewPoint)
        return CGPoint(x: min(max(value.x, 0), 1), y: min(max(value.y, 0), 1))
    }
}
