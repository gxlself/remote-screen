import QuartzCore
import UIKit

/// Owns local zoom state. It never calls the remote input ABI; the transform
/// is applied to the local content layer; RemoteSessionSurface mirrors the
/// same transform onto the cursor overlay.
final class SessionZoomController: NSObject {
    private weak var transformLayer: CALayer?
    private(set) var viewport = SessionViewport()
    private(set) var viewBounds: CGRect = .zero
    private(set) var contentRect: CGRect = .zero
    private var lastViewportEmission: CFTimeInterval = -.greatestFiniteMagnitude
    private var pendingViewportEmission: DispatchWorkItem?
    private var viewportEmissionGeneration: UInt64 = 0
    private var animationGeneration: UInt64 = 0
    private var animating = false
    private var pinching = false
    private var activePans: Set<ObjectIdentifier> = []
    private(set) var isInteracting = false
    private var lastPinchScale: CGFloat = 1
    private var lastPinchLocation: CGPoint = .zero
    private var keyboardTranslation: CGPoint = .zero

    var reduceMotion = false
    var keyboardInset: CGFloat = 0
    var onViewport: ((SessionViewport) -> Void)?

    init(layer: CALayer) {
        transformLayer = layer
        super.init()
    }

    /// Keyboard centering can translate the layer past a clamped image edge.
    /// Include that translation when intersecting the image with view bounds,
    /// while keeping the JS minimap and remote input's viewport unchanged.
    var reportingViewport: SessionViewport {
        var result = viewport
        result.contentRect = result.contentRect.offsetBy(dx: keyboardTranslation.x, dy: keyboardTranslation.y)
        return result
    }

    func observeGesture(_ gesture: UIGestureRecognizer) {
        guard gesture is UIPanGestureRecognizer || gesture is UIPinchGestureRecognizer else { return }
        gesture.removeTarget(self, action: #selector(gestureDidChange(_:)))
        gesture.addTarget(self, action: #selector(gestureDidChange(_:)))
    }

    func stopObservingGesture(_ gesture: UIGestureRecognizer) {
        gesture.removeTarget(self, action: #selector(gestureDidChange(_:)))
        activePans.remove(ObjectIdentifier(gesture))
        if gesture is UIPinchGestureRecognizer { pinching = false }
        updateInteraction()
    }

    @objc private func gestureDidChange(_ gesture: UIGestureRecognizer) {
        let active = gesture.state == .began || gesture.state == .changed
        if gesture is UIPinchGestureRecognizer {
            pinching = active
        } else if active {
            activePans.insert(ObjectIdentifier(gesture))
        } else {
            activePans.remove(ObjectIdentifier(gesture))
        }
        updateInteraction()
    }

    private func updateInteraction() {
        let next = pinching || !activePans.isEmpty || animating
        guard next != isInteracting else { return }
        isInteracting = next
        emitViewport(force: true)
    }

    func updateLayout(contentRect: CGRect, viewBounds: CGRect) {
        if self.viewBounds.size != viewBounds.size {
            viewport = SessionZoomMath.relayout(viewport: viewport, oldSize: self.viewBounds.size,
                                               newSize: viewBounds.size)
            keyboardTranslation = .zero
        }
        self.contentRect = contentRect
        self.viewBounds = viewBounds
        viewport.contentRect = contentRect
        viewport.offset = SessionZoomMath.clampOffset(viewport.offset, zoom: viewport.zoom)
        applyTransform(animated: false)
    }

    func pinch(scale: CGFloat, location: CGPoint, state: UIGestureRecognizer.State) {
        pinching = state == .began || state == .changed
        updateInteraction()
        guard scale.isFinite, scale > 0 else { return }
        if state == .began {
            lastPinchScale = scale
            lastPinchLocation = location
            return
        }
        let factor = scale / max(lastPinchScale, 0.0001)
        let inputLocation = CGPoint(x: location.x - keyboardTranslation.x, y: location.y - keyboardTranslation.y)
        let anchor = viewport.normalized(fromView: inputLocation)
        viewport = viewport.zoomed(by: factor, anchor: anchor)
        let delta = CGPoint(x: location.x - lastPinchLocation.x, y: location.y - lastPinchLocation.y)
        if delta != .zero {
            viewport = viewport.panned(by: CGPoint(
                x: -viewport.normalizedDelta(fromViewDelta: delta).x,
                y: -viewport.normalizedDelta(fromViewDelta: delta).y))
        }
        lastPinchScale = scale
        lastPinchLocation = location
        applyTransform(animated: false)
        if state == .ended || state == .cancelled || state == .failed {
            lastPinchScale = 1
            emitViewport(force: true)
        } else {
            emitViewport(force: false)
        }
    }

    func endPinch() {
        pinching = false
        updateInteraction()
        lastPinchScale = 1
        emitViewport(force: true)
    }

    func followCursor(_ cursor: CGPoint, margin: CGFloat = SessionZoomMath.defaultFollowMargin) {
        guard viewport.zoom > 1 else { return }
        var next = SessionZoomMath.followCursor(cursor: cursor, viewport: viewport, margin: margin)
        if keyboardInset > 0, contentRect.width > 0, contentRect.height > 0 {
            let point = screenPoint(forNormalized: cursor)
            let availableBottom = max(viewBounds.minY + 1, viewBounds.maxY - keyboardInset)
            let marginY = max(1, (availableBottom - viewBounds.minY) * margin)
            let top = viewBounds.minY + marginY
            let bottom = availableBottom - marginY
            var offset = next.offset
            if point.y < top {
                offset.y -= (top - point.y) / (contentRect.height * viewport.zoom)
            } else if point.y > bottom {
                offset.y += (point.y - bottom) / (contentRect.height * viewport.zoom)
            }
            next = SessionViewport(contentRect: contentRect, zoom: viewport.zoom, offset: offset)
        }
        guard next.offset != viewport.offset else { return }
        viewport = next
        applyTransform(animated: false)
        emitViewport(force: false)
    }

    func pan(by viewDelta: CGPoint, emit: Bool = true) {
        let delta = viewport.normalizedDelta(fromViewDelta: viewDelta)
        viewport = viewport.panned(by: CGPoint(x: -delta.x, y: -delta.y))
        applyTransform(animated: false)
        if emit { emitViewport(force: false) }
    }

    func reset() {
        pinching = false
        activePans.removeAll()
        keyboardTranslation = .zero
        viewport = SessionViewport(contentRect: contentRect)
        applyTransform(animated: !reduceMotion)
        emitViewport(force: true)
    }

    func setKeyboardTarget(cursor: CGPoint, inset: CGFloat, minZoom: CGFloat = 1.7,
                           animated: Bool = true, emit: Bool = true) {
        keyboardInset = max(0, inset)
        guard contentRect.width > 0, contentRect.height > 0, viewBounds.width > 0, viewBounds.height > 0 else { return }
        let targetZoom = viewport.zoom < minZoom ? minZoom : viewport.zoom
        let availableHeight = max(viewBounds.height - keyboardInset, 1)
        let center = CGPoint(x: viewBounds.midX, y: viewBounds.minY + availableHeight / 2)
        let targetOffset = CGPoint(
            x: cursor.x - (center.x - contentRect.minX) / (contentRect.width * targetZoom),
            y: cursor.y - (center.y - contentRect.minY) / (contentRect.height * targetZoom))
        viewport = SessionViewport(contentRect: contentRect, zoom: targetZoom, offset: targetOffset)
        // SessionViewport keeps its visible rectangle inside the remote image.
        // When the cursor is close to an image edge, a small physical layer
        // translation still lets it sit in the keyboard-safe center without
        // changing the normalized viewport contract.
        keyboardTranslation = CGPoint(
            x: (viewport.offset.x - targetOffset.x) * contentRect.width * targetZoom,
            y: (viewport.offset.y - targetOffset.y) * contentRect.height * targetZoom)
        applyTransform(animated: animated && !reduceMotion, duration: 0.25)
        if emit { emitViewport(force: true) }
    }

    func restore(_ saved: SessionViewport) {
        keyboardInset = 0
        keyboardTranslation = .zero
        viewport = SessionViewport(contentRect: contentRect, zoom: saved.zoom, offset: saved.offset)
        applyTransform(animated: !reduceMotion, duration: 0.25)
        emitViewport(force: true)
    }

    func screenPoint(forNormalized point: CGPoint) -> CGPoint {
        // The layer transform maps unzoomed content coordinates. Viewport's
        // viewPoint already includes zoom, so use the base content point here.
        let local = CGPoint(x: contentRect.minX + point.x * contentRect.width,
                            y: contentRect.minY + point.y * contentRect.height)
        return local.applying(currentTransform())
    }

    func currentTransform() -> CGAffineTransform {
        let z = viewport.zoom
        return CGAffineTransform(a: z, b: 0, c: 0, d: z,
                                 tx: contentRect.minX * (1 - z) - viewport.offset.x * contentRect.width * z + keyboardTranslation.x,
                                 ty: contentRect.minY * (1 - z) - viewport.offset.y * contentRect.height * z + keyboardTranslation.y)
    }

    func emitViewport(force: Bool) {
        guard let onViewport else { return }
        let now = CACurrentMediaTime()
        if force || now - lastViewportEmission >= 0.1 {
            viewportEmissionGeneration &+= 1
            pendingViewportEmission?.cancel()
            pendingViewportEmission = nil
            lastViewportEmission = now
            onViewport(viewport)
        } else if pendingViewportEmission == nil {
            let expected = viewportEmissionGeneration
            let work = DispatchWorkItem { [weak self] in
                guard let self, self.viewportEmissionGeneration == expected,
                      self.pendingViewportEmission != nil else { return }
                self.pendingViewportEmission = nil
                self.emitViewport(force: false)
            }
            pendingViewportEmission = work
            DispatchQueue.main.asyncAfter(deadline: .now() + max(0, lastViewportEmission + 0.1 - now),
                                          execute: work)
        }
    }

    private func applyTransform(animated: Bool, duration: TimeInterval = 0.2) {
        guard let transformLayer else { return }
        let target = currentTransform()
        let old = transformLayer.presentation()?.affineTransform() ?? transformLayer.affineTransform()
        let shouldAnimate = animated && !reduceMotion && old != target
        animationGeneration &+= 1
        let expected = animationGeneration
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        transformLayer.removeAnimation(forKey: "rs.zoom")
        transformLayer.setAffineTransform(target)
        animating = shouldAnimate
        if shouldAnimate {
            CATransaction.setCompletionBlock { [weak self] in
                guard let self, self.animationGeneration == expected else { return }
                self.animating = false
                self.updateInteraction()
            }
            let animation = CABasicAnimation(keyPath: "transform")
            animation.fromValue = NSValue(cgAffineTransform: old)
            animation.toValue = NSValue(cgAffineTransform: target)
            animation.duration = duration
            animation.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            transformLayer.add(animation, forKey: "rs.zoom")
        }
        CATransaction.commit()
        updateInteraction()
    }

    deinit { pendingViewportEmission?.cancel() }
}
