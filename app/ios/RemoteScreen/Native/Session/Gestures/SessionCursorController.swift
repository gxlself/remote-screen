import ImageIO
import UIKit

final class SessionCursorController: SessionCursorSink {
    let layer = SessionCursorLayer()
    private(set) var viewport = SessionViewport()
    private var layoutViewport = SessionViewport()
    var position: CGPoint { model.position }

    private struct Shape {
        let image: CGImage
        let hotspot: CGPoint
        let scale: CGFloat
    }

    private let decodeQueue = DispatchQueue(label: "rs.session.cursor-decode", qos: .userInitiated)
    private var model = TrackpadCursorModel()
    private var arbiter = TrackpadHostCursorArbiter()
    private var shapes: [UInt64: Shape] = [:]
    private var recentShapeIds: [UInt64] = []
    private var requestedShapeId: UInt64?
    private var shape: Shape?
    private var generation: UInt64 = 0
    private var videoSize: CGSize = .zero
    private var contentTransform = CGAffineTransform.identity
    private var attached = false
    private var hostVisible = true

    func setAttached(_ attached: Bool) {
        self.attached = attached
        refresh()
    }

    func update(viewport: SessionViewport, videoSize: CGSize) {
        layoutViewport = viewport
        self.videoSize = videoSize
        updateInputViewport()
        refresh()
    }

    func updateTransform(_ transform: CGAffineTransform) {
        contentTransform = transform
        updateInputViewport()
        refresh()
    }

    private func updateInputViewport() {
        viewport = layoutViewport
        let origin = layoutViewport.contentRect.origin.applying(contentTransform)
        let expected = layoutViewport.viewPoint(fromNormalized: .zero)
        // TouchTapMapper also needs the physical keyboard-safe translation.
        // Keep the reported viewport normalized; shift only this input map.
        viewport.contentRect = layoutViewport.contentRect.offsetBy(dx: origin.x - expected.x,
                                                                  dy: origin.y - expected.y)
    }

    @discardableResult
    func move(viewDelta: CGPoint, gain: CGFloat, now: TimeInterval) -> CGPoint {
        model.apply(viewDelta: viewDelta, viewport: viewport, gain: gain)
        arbiter.recordLocalMove(at: now)
        refresh()
        return position
    }

    @discardableResult
    func moveTo(normalized: CGPoint, now: TimeInterval) -> CGPoint {
        model.move(to: normalized)
        arbiter.recordLocalMove(at: now)
        refresh()
        return position
    }

    func center(now: TimeInterval) {
        model.move(to: CGPoint(x: 0.5, y: 0.5))
        arbiter.recordLocalMove(at: now)
        refresh()
    }

    func reset() {
        generation &+= 1
        attached = false
        hostVisible = true
        model = TrackpadCursorModel()
        arbiter = TrackpadHostCursorArbiter()
        shapes.removeAll()
        recentShapeIds.removeAll()
        requestedShapeId = nil
        shape = nil
        refresh()
    }

    func consume(_ event: SessionEvent) {
        precondition(Thread.isMainThread)
        switch event.kind {
        case RS_EV_STREAM_CONFIG, RS_EV_CURSOR_POS:
            hostVisible = event.cursorVisible
            if event.kind == RS_EV_STREAM_CONFIG || arbiter.shouldAcceptHost(at: CACurrentMediaTime()) {
                model.move(to: CGPoint(x: CGFloat(event.cursorX), y: CGFloat(event.cursorY)))
            }
            refresh()
        case RS_EV_CURSOR_SHAPE:
            decode(event)
        case RS_EV_CURSOR_USE:
            requestedShapeId = event.shapeId
            if let cached = shapes[event.shapeId] {
                touch(event.shapeId)
                shape = cached
                refresh()
            }
        case RS_EV_CURSOR_HIDDEN:
            hostVisible = event.cursorVisible
            refresh()
        default:
            break
        }
    }

    private func decode(_ event: SessionEvent) {
        let previousShapeId = requestedShapeId
        requestedShapeId = event.shapeId
        if let cached = shapes[event.shapeId] {
            touch(event.shapeId)
            shape = cached
            refresh()
            return
        }
        let expected = generation
        decodeQueue.async { [weak self] in
            let options = [kCGImageSourceShouldCache: true, kCGImageSourceShouldCacheImmediately: true] as CFDictionary
            guard let source = CGImageSourceCreateWithData(event.data as CFData, nil),
                  CGImageSourceGetType(source) == "public.png" as CFString,
                  let image = CGImageSourceCreateImageAtIndex(source, 0, options) else {
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.generation == expected, self.requestedShapeId == event.shapeId else { return }
                    self.requestedShapeId = previousShapeId
                    if let previousShapeId, let cached = self.shapes[previousShapeId] {
                        self.shape = cached
                        self.refresh()
                    }
                }
                return
            }
            let shape = Shape(image: image,
                              hotspot: CGPoint(x: min(CGFloat(event.hotX), CGFloat(image.width)),
                                               y: min(CGFloat(event.hotY), CGFloat(image.height))),
                              scale: event.scale.isFinite && event.scale > 0 ? CGFloat(event.scale) : 1)
            DispatchQueue.main.async { [weak self] in
                guard let self, self.generation == expected else { return }
                self.shapes[event.shapeId] = shape
                self.touch(event.shapeId)
                if self.recentShapeIds.count > 64 {
                    self.shapes.removeValue(forKey: self.recentShapeIds.removeFirst())
                }
                if self.requestedShapeId == event.shapeId {
                    self.shape = shape
                    self.refresh()
                }
            }
        }
    }

    private func touch(_ id: UInt64) {
        recentShapeIds.removeAll { $0 == id }
        recentShapeIds.append(id)
    }

    private func refresh() {
        var size = CGSize(width: 16, height: 24)
        var hotspot = CGPoint.zero
        if let shape {
            let ratio = videoSize.width > 0 ? layoutViewport.contentRect.width / videoSize.width : 1 / shape.scale
            size = CGSize(width: min(max(CGFloat(shape.image.width) * ratio, 12), 64),
                          height: min(max(CGFloat(shape.image.height) * ratio, 12), 64))
            hotspot = CGPoint(x: shape.hotspot.x * size.width / CGFloat(shape.image.width),
                              y: shape.hotspot.y * size.height / CGFloat(shape.image.height))
        }
        let rect = layoutViewport.contentRect
        let tip = CGPoint(x: rect.minX + position.x * rect.width,
                          y: rect.minY + position.y * rect.height).applying(contentTransform)
        let hasContent = rect.width > 0 && rect.height > 0
        let visible = layoutViewport.visibleRect
        let inViewport = position.x >= visible.minX && position.x <= visible.maxX
            && position.y >= visible.minY && position.y <= visible.maxY
        // Position is already in screen points. Scale the glyph and hotspot
        // once; applying the content translation to the layer a second time
        // would drift the cursor after repeated layout/magnifier refreshes.
        let zoom = layoutViewport.zoom
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layer.setAffineTransform(CGAffineTransform(scaleX: zoom, y: zoom))
        layer.update(image: shape?.image, size: size,
                     hotspot: CGPoint(x: hotspot.x * zoom, y: hotspot.y * zoom), tip: tip,
                     hidden: !attached || !hostVisible || !hasContent || !inViewport)
        CATransaction.commit()
    }
}
