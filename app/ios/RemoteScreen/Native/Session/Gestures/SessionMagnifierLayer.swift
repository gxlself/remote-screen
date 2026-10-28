import CoreImage
import CoreVideo
import UIKit

/// A local 2x magnifier overlay. It samples the most recent decoded frame and
/// never participates in remote input or video decoding.
final class SessionMagnifierLayer: CALayer {
    private final class DisplayLinkTarget: NSObject {
        weak var owner: SessionMagnifierLayer?
        @objc func tick(_ link: CADisplayLink) { owner?.render() }
    }

    private weak var pipeline: SessionVideoPipeline?
    private let context = CIContext(options: [.cacheIntermediates: false])
    private let target = DisplayLinkTarget()
    private var displayLink: CADisplayLink?
    private let crosshair = CAShapeLayer()
    private let hint = CATextLayer()
    private var cursor = CGPoint(x: 0.5, y: 0.5)
    private var cursorViewPoint = CGPoint.zero
    private var viewport = SessionViewport()
    private var videoSize = CGSize.zero
    private var src = CGRect(x: 0, y: 0, width: 1, height: 1)
    private var backdrop: CGImage?
    private var viewBounds = CGRect.zero
    private var pointsPerVideoPixel: CGFloat = 1
    private var reduceMotion = false
    private var requestedVisible = false

    init(pipeline: SessionVideoPipeline? = nil) {
        self.pipeline = pipeline
        super.init()
        target.owner = self
        anchorPoint = .zero
        contentsScale = UIScreen.main.scale
        backgroundColor = UIColor.black.cgColor
        cornerRadius = 60
        masksToBounds = true
        borderColor = UIColor.white.cgColor
        borderWidth = 2
        shadowColor = UIColor.black.cgColor
        shadowOpacity = 0.45
        shadowRadius = 6
        shadowOffset = CGSize(width: 0, height: 3)
        zPosition = 20

        let path = CGMutablePath()
        path.move(to: CGPoint(x: 60, y: 50))
        path.addLine(to: CGPoint(x: 60, y: 70))
        path.move(to: CGPoint(x: 50, y: 60))
        path.addLine(to: CGPoint(x: 70, y: 60))
        crosshair.path = path
        crosshair.strokeColor = UIColor.white.cgColor
        crosshair.lineWidth = 1.5
        crosshair.fillColor = UIColor.clear.cgColor
        crosshair.contentsScale = contentsScale
        addSublayer(crosshair)

        hint.string = "松手单击"
        hint.font = UIFont.systemFont(ofSize: 11, weight: .medium)
        hint.fontSize = 11
        hint.alignmentMode = .center
        hint.foregroundColor = UIColor.white.cgColor
        hint.contentsScale = contentsScale
        hint.frame = CGRect(x: 5, y: 92, width: 110, height: 18)
        addSublayer(hint)
        isHidden = true
    }

    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { return nil }

    func setPipeline(_ pipeline: SessionVideoPipeline?) { self.pipeline = pipeline }

    func update(cursor: CGPoint, viewport: SessionViewport, videoSize: CGSize, src: CGRect, backdrop: CGImage?,
                cursorViewPoint: CGPoint? = nil,
                frame: CGRect, pointsPerVideoPixel: CGFloat, reduceMotion: Bool, pixelBuffer: CVPixelBuffer? = nil) {
        precondition(Thread.isMainThread)
        self.cursor = cursor
        self.cursorViewPoint = cursorViewPoint ?? viewport.viewPoint(fromNormalized: cursor)
        self.viewport = viewport
        self.videoSize = videoSize
        self.src = src
        self.backdrop = backdrop
        self.viewBounds = frame
        self.pointsPerVideoPixel = pointsPerVideoPixel
        self.reduceMotion = reduceMotion
        let circle = SessionMagnifierGeometry.frame(finger: self.cursorViewPoint, viewBounds: frame,
                                                     diameter: 120, offset: CGPoint(x: 0, y: -80))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        self.frame = circle
        CATransaction.commit()
        if requestedVisible { render(pixelBuffer: pixelBuffer) }
    }

    func show(_ on: Bool) {
        precondition(Thread.isMainThread)
        requestedVisible = on
        isHidden = !on
        if on {
            startDisplayLink()
            render()
        } else {
            stopDisplayLink()
        }
    }

    private func startDisplayLink() {
        guard displayLink == nil else { return }
        let link = CADisplayLink(target: target, selector: #selector(DisplayLinkTarget.tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 30, preferred: 30)
        displayLink = link
        link.add(to: .main, forMode: .common)
    }

    private func stopDisplayLink() {
        displayLink?.invalidate()
        displayLink = nil
    }

    private func render(pixelBuffer: CVPixelBuffer? = nil) {
        guard requestedVisible else { return }
        let cropCursor = ViewportGeometry.cursorInCrop(cursor, src: src)
        let size: CGSize
        let sampleCursor: CGPoint
        let ppp: CGFloat
        var image: CIImage
        var samplingCrop = false
        if let cropCursor, let buffer = pixelBuffer ?? pipeline?.latestPixelBuffer() {
            size = CGSize(width: CVPixelBufferGetWidth(buffer), height: CVPixelBufferGetHeight(buffer))
            sampleCursor = cropCursor
            ppp = videoSize.width > 0 ? pointsPerVideoPixel * videoSize.width / size.width : pointsPerVideoPixel
            image = CIImage(cvPixelBuffer: buffer)
            samplingCrop = !ViewportGeometry.isFull(src)
        } else if let backdrop {
            size = CGSize(width: backdrop.width, height: backdrop.height)
            sampleCursor = cursor
            ppp = viewport.contentRect.width * viewport.zoom / size.width
            image = CIImage(cgImage: backdrop)
        } else {
            contents = nil
            isHidden = true
            return
        }
        var source = SessionMagnifierGeometry.sourceRect(
            cursorNormalized: sampleCursor, videoSize: size,
            zoom: 2, diameterPt: 120, pointsPerVideoPixel: ppp)
        if samplingCrop {
            // Clamp the lens at display edges, not crop edges. Fill the part
            // of the lens outside the decoded image from the same backdrop,
            // so its center does not slide when the cursor crosses a crop.
            let logical = CGSize(width: size.width / src.width, height: size.height / src.height)
            let wholeSource = SessionMagnifierGeometry.sourceRect(
                cursorNormalized: cursor, videoSize: logical,
                zoom: 2, diameterPt: 120, pointsPerVideoPixel: ppp)
            source = wholeSource.offsetBy(dx: -src.minX * logical.width, dy: -src.minY * logical.height)
            if let backdrop {
                let background = CIImage(cgImage: backdrop)
                    .transformed(by: CGAffineTransform(scaleX: logical.width / CGFloat(backdrop.width),
                                                       y: logical.height / CGFloat(backdrop.height)))
                    .transformed(by: CGAffineTransform(translationX: -src.minX * logical.width,
                                                       y: -(1 - src.maxY) * logical.height))
                image = image.composited(over: background)
            }
        }
        // CIImage is bottom-left-origin; geometry is top-left-origin.
        let crop = CGRect(x: source.minX, y: size.height - source.maxY,
                          width: source.width, height: source.height)
        guard let cgImage = context.createCGImage(image.cropped(to: crop), from: crop) else {
            contents = nil
            isHidden = true
            return
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        contents = cgImage
        isHidden = false
        backgroundColor = UIColor.clear.cgColor
        CATransaction.commit()
    }

    deinit { stopDisplayLink() }
}

private extension CGRect {
    var center: CGPoint { CGPoint(x: midX, y: midY) }
}
