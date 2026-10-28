import AVFoundation
@preconcurrency import AppKit
import QuartzCore

/// The AppKit surface used by the desktop session window.
///
/// The session window owns the view and calls its public methods on the main
/// thread.  The headless test surface is deliberately allowed to be created
/// and updated from the calling thread because it is never attached to an
/// AppKit window.
@preconcurrency
public final class RsDtVideoView: NSView {
    private let displayLayer = AVSampleBufferDisplayLayer()
    private let overlayLayer = CATextLayer()
    private let cursorLayer = CALayer()
    private let cursorPointLock = NSLock()
    private var videoSize: CGSize = .zero
    private var overlayText: String?
    private var remoteCursorShown = false
    private var normalizedCursor = CGPoint.zero
    private var remoteCursorPosition = CGPoint.zero

    /// Remote pointer coordinates start at the top-left, like the wire format.
    public override var isFlipped: Bool { true }
    public override var isOpaque: Bool { true }

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        configureLayers()
    }

    public required init?(coder: NSCoder) {
        super.init(coder: coder)
        wantsLayer = true
        configureLayers()
    }

    private func configureLayers() {
        layer = CALayer()
        guard let layer else { return }
        layer.isGeometryFlipped = true
        layer.backgroundColor = NSColor.black.cgColor
        layer.masksToBounds = true

        displayLayer.videoGravity = .resizeAspect
        displayLayer.backgroundColor = NSColor.black.cgColor
        layer.addSublayer(displayLayer)

        overlayLayer.alignmentMode = .center
        overlayLayer.isWrapped = false
        overlayLayer.truncationMode = .end
        overlayLayer.contentsScale = 2
        overlayLayer.font = NSFont.systemFont(ofSize: 15)
        overlayLayer.fontSize = 15
        overlayLayer.foregroundColor = NSColor.white.cgColor
        overlayLayer.backgroundColor = NSColor.black.withAlphaComponent(0.62).cgColor
        overlayLayer.cornerRadius = 4
        overlayLayer.masksToBounds = true
        overlayLayer.isHidden = true
        layer.addSublayer(overlayLayer)

        cursorLayer.contentsScale = 2
        cursorLayer.contentsGravity = .resize
        cursorLayer.isHidden = true
        layer.addSublayer(cursorLayer)
        setCursorImage(png: nil, hotX: 0, hotY: 0, scale: 1)
        layoutVideoLayers()
    }

    public override func layout() {
        super.layout()
        layoutVideoLayers()
    }

    public override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        let scale = window?.backingScaleFactor ?? 2
        overlayLayer.contentsScale = scale
        cursorLayer.contentsScale = scale
    }

    private func layoutVideoLayers() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        displayLayer.frame = bounds
        let textWidth = (overlayText ?? "").size(withAttributes: [.font: NSFont.systemFont(ofSize: 15)]).width
        let width = min(max(0, bounds.width - 32), max(80, ceil(textWidth) + 24))
        overlayLayer.frame = CGRect(x: bounds.midX - width / 2, y: bounds.midY - 14,
                                     width: width, height: 28)
        updateCursorPosition()
        CATransaction.commit()
    }

    /// The AVFoundation display layer used by the video pipeline.
    public var videoDisplayLayer: AVSampleBufferDisplayLayer { displayLayer }

    /// Set the dimensions of the decoded video.  The caller owns the main
    /// thread rule for a window-backed view.
    public func setVideoSize(width: UInt32, height: UInt32) {
        videoSize = CGSize(width: CGFloat(width), height: CGFloat(height))
        // Direct layout also works for a headless NSView without a RunLoop.
        layoutVideoLayers()
    }

    public func setOverlayText(_ text: String?) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        overlayText = text
        overlayLayer.string = text
        overlayLayer.isHidden = text == nil || text?.isEmpty == true
        layoutVideoLayers()
        CATransaction.commit()
    }

    /// The rectangle occupied by the decoded image after aspect-fit scaling.
    public var videoContentRect: CGRect {
        guard videoSize.width > 0, videoSize.height > 0 else { return .zero }
        return AVMakeRect(aspectRatio: videoSize, insideRect: bounds)
    }

    public func setCursorImage(png: Data?, hotX: UInt32, hotY: UInt32, scale: Float) {
        let cgImage: CGImage?
        let size: CGSize
        let hotSpot: CGPoint
        if let png, !png.isEmpty, scale.isFinite, scale > 0,
           let image = NSImage(data: png),
           let decoded = image.cgImage(forProposedRect: nil, context: nil, hints: nil) {
            let pointScale = CGFloat(scale)
            cgImage = decoded
            size = CGSize(width: CGFloat(decoded.width) / pointScale,
                          height: CGFloat(decoded.height) / pointScale)
            hotSpot = CGPoint(x: CGFloat(hotX) / pointScale, y: CGFloat(hotY) / pointScale)
        } else {
            // `WindowCommand::SetCursorImage(None)` means the system arrow.
            let arrow = NSCursor.arrow
            cgImage = arrow.image.cgImage(forProposedRect: nil, context: nil, hints: nil)
            size = arrow.image.size
            hotSpot = arrow.hotSpot
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        cursorLayer.contents = cgImage
        cursorLayer.bounds = CGRect(origin: .zero, size: size)
        cursorLayer.anchorPoint = CGPoint(
            x: size.width > 0 ? hotSpot.x / size.width : 0,
            y: size.height > 0 ? hotSpot.y / size.height : 0
        )
        cursorLayer.isHidden = !remoteCursorShown || cgImage == nil
        updateCursorPosition()
        CATransaction.commit()
    }

    public func showRemoteCursor(_ shown: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        remoteCursorShown = shown
        cursorLayer.isHidden = !shown || cursorLayer.contents == nil
        CATransaction.commit()
    }

    public func moveRemoteCursor(nx: Double, ny: Double) {
        normalizedCursor = CGPoint(x: CGFloat(nx.isFinite ? min(1, max(0, nx)) : 0),
                                    y: CGFloat(ny.isFinite ? min(1, max(0, ny)) : 0))
        updateCursorPosition()
    }

    /// The last normalized cursor position converted to view coordinates.
    public var remoteCursorPoint: CGPoint {
        cursorPointLock.lock()
        defer { cursorPointLock.unlock() }
        return remoteCursorPosition
    }

    private func updateCursorPosition() {
        let rect = videoContentRect
        let point = CGPoint(x: rect.minX + normalizedCursor.x * rect.width,
                             y: rect.minY + normalizedCursor.y * rect.height)
        cursorPointLock.lock()
        remoteCursorPosition = point
        cursorPointLock.unlock()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        cursorLayer.position = point
        CATransaction.commit()
    }
}
