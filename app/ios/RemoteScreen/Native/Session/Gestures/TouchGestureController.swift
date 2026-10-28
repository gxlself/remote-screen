import UIKit

/// Gesture recognizers used by iPad's direct-touch mode. Coordinates are
/// converted locally and sent through the same serialized input sender as the
/// trackpad controller.
final class TouchGestureController: NSObject, UIGestureRecognizerDelegate {
    private final class MomentumTarget: NSObject {
        weak var controller: TouchGestureController?
        @objc func tick(_ link: CADisplayLink) { controller?.stepMomentum(link) }
    }

    private weak var surface: RemoteSessionSurface?
    private let input: SessionInputSender
    private let cursor: SessionCursorController
    private var recognizers: [UIGestureRecognizer] = []
    private var installed = false
    private var suspended = false
    private var dragging = false
    private var magnifying = false
    private var magnifierCancelled = false
    private var scrolling = false
    private var pinching = false
    private var clicks = TrackpadClickCounter()
    private var momentum = TrackpadScrollMomentum()
    private let momentumTarget = MomentumTarget()
    private var displayLink: CADisplayLink?
    private var lastMomentumTime: CFTimeInterval?
    private let feedback = UIImpactFeedbackGenerator(style: .light)

    init(surface: RemoteSessionSurface, input: SessionInputSender, cursor: SessionCursorController) {
        self.surface = surface
        self.input = input
        self.cursor = cursor
        super.init()
        momentumTarget.controller = self

        let pan = UIPanGestureRecognizer(target: self, action: #selector(onPan(_:)))
        pan.minimumNumberOfTouches = 1
        pan.maximumNumberOfTouches = 1
        let scroll = UIPanGestureRecognizer(target: self, action: #selector(onScroll(_:)))
        scroll.minimumNumberOfTouches = 2
        scroll.maximumNumberOfTouches = 2
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(onPinch(_:)))
        let tap = UITapGestureRecognizer(target: self, action: #selector(onTap(_:)))
        tap.numberOfTouchesRequired = 1
        let rightTap = UITapGestureRecognizer(target: self, action: #selector(onRightTap(_:)))
        rightTap.numberOfTouchesRequired = 2
        let threeFingerTap = UITapGestureRecognizer(target: self, action: #selector(onThreeFingerTap(_:)))
        threeFingerTap.numberOfTouchesRequired = 3
        let twoFingerDoubleTap = UITapGestureRecognizer(target: self, action: #selector(onTwoFingerDoubleTap(_:)))
        twoFingerDoubleTap.numberOfTouchesRequired = 2
        twoFingerDoubleTap.numberOfTapsRequired = 2
        rightTap.require(toFail: twoFingerDoubleTap)
        let longPress = UILongPressGestureRecognizer(target: self, action: #selector(onLongPress(_:)))
        longPress.minimumPressDuration = 0.5
        recognizers = [pan, scroll, pinch, tap, rightTap, threeFingerTap, twoFingerDoubleTap, longPress]
        for recognizer in recognizers {
            recognizer.delegate = self
            if !(recognizer is UIPinchGestureRecognizer) {
                recognizer.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
            }
        }
    }

    func install() {
        guard !installed, let surface else { return }
        installed = true
        for recognizer in recognizers {
            recognizer.isEnabled = !suspended
            surface.addGestureRecognizer(recognizer)
        }
    }

    func uninstall() {
        cancel()
        guard installed else { return }
        installed = false
        for recognizer in recognizers { surface?.removeGestureRecognizer(recognizer) }
    }

    func setSuspended(_ suspended: Bool) {
        self.suspended = suspended
        if suspended { cancel() }
        for recognizer in recognizers { recognizer.isEnabled = !suspended }
    }

    func cancel() {
        let hadDrag = dragging
        let hadScroll = scrolling
        let hadMagnifier = magnifying
        dragging = false
        magnifying = false
        magnifierCancelled = false
        scrolling = false
        pinching = false
        if hadDrag { input.button(0, down: false, at: cursor.position) }
        if hadScroll { input.scroll(dx: 0, dy: 0, phase: 3) }
        stopMomentum()
        input.releaseAll()
        clicks = TrackpadClickCounter()
        surface?.setMagnifierVisible(false)
        for recognizer in recognizers {
            recognizer.isEnabled = false
            recognizer.isEnabled = !suspended
        }
        if hadDrag { surface?.reportGesture("drag") }
        if hadScroll { surface?.reportGesture("scroll") }
        if hadMagnifier { surface?.reportGesture("magnifier") }
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        gestureRecognizer is UIPinchGestureRecognizer || other is UIPinchGestureRecognizer ||
            (gestureRecognizer is UIPanGestureRecognizer && other is UILongPressGestureRecognizer) ||
            (gestureRecognizer is UILongPressGestureRecognizer && other is UIPanGestureRecognizer)
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        guard surface?.pencilActive != true else { return false }
        stopMomentum()
        if canInteract && surface?.prefs.haptics == true { feedback.prepare() }
        return canInteract
    }

    private var canInteract: Bool { installed && !suspended && surface?.sessionInputReady == true }

    @objc private func onPinch(_ recognizer: UIPinchGestureRecognizer) {
        guard canInteract else { return }
        if recognizer.state == .began {
            pinching = true
            if dragging {
                dragging = false
                input.button(0, down: false, at: cursor.position)
            }
            if magnifying {
                magnifying = false
                surface?.setMagnifierVisible(false)
            }
            if scrolling {
                scrolling = false
                input.scroll(dx: 0, dy: 0, phase: 3)
            }
        }
        surface?.handlePinch(scale: recognizer.scale, location: recognizer.location(in: surface), state: recognizer.state,
                             report: recognizer.state == .ended || recognizer.state == .cancelled)
        if recognizer.state == .ended || recognizer.state == .cancelled || recognizer.state == .failed {
            pinching = false
        }
    }

    @objc private func onPan(_ recognizer: UIPanGestureRecognizer) {
        guard canInteract else { return }
        let location = recognizer.location(in: surface)
        switch recognizer.state {
        case .began:
            // A long press owns the pointer once the magnifier is active. The
            // one-finger pan may begin at the same time as the long press when
            // the user starts dragging, but it must not press the left button.
            guard !magnifying else { return }
            dragging = true
            movePointer(to: location)
            input.button(0, down: true, at: cursor.position)
            impactFeedback()
        case .changed:
            if dragging && !magnifying { movePointer(to: location) }
        case .ended, .cancelled:
            guard dragging else { return }
            if recognizer.state == .ended { movePointer(to: location) }
            dragging = false
            input.flushPointer()
            input.button(0, down: false, at: cursor.position)
            surface?.finishViewportGesture()
            surface?.reportGesture("drag")
        default:
            break
        }
    }

    @objc private func onTap(_ recognizer: UITapGestureRecognizer) {
        guard canInteract, recognizer.state == .ended else { return }
        let point = recognizer.location(in: surface)
        movePointer(to: point)
        let count = clicks.register(position: cursor.position, time: CACurrentMediaTime())
        input.click(0, at: cursor.position, clicks: UInt8(count))
        impactFeedback()
        surface?.finishViewportGesture()
        surface?.reportGesture(count >= 2 ? "doubleClick" : "click")
    }

    @objc private func onRightTap(_ recognizer: UITapGestureRecognizer) {
        guard canInteract, recognizer.state == .ended else { return }
        movePointer(to: recognizer.location(in: surface))
        input.click(1, at: cursor.position)
        impactFeedback()
        surface?.finishViewportGesture()
        surface?.reportGesture("rightClick")
    }

    @objc private func onThreeFingerTap(_ recognizer: UITapGestureRecognizer) {
        guard canInteract, recognizer.state == .ended else { return }
        surface?.reportGesture("threeFingerTap")
    }

    @objc private func onTwoFingerDoubleTap(_ recognizer: UITapGestureRecognizer) {
        guard canInteract, recognizer.state == .ended else { return }
        surface?.reportGesture("twoFingerDoubleTap")
    }

    @objc private func onLongPress(_ recognizer: UILongPressGestureRecognizer) {
        guard canInteract else { return }
        let location = recognizer.location(in: surface)
        switch recognizer.state {
        case .began:
            if dragging {
                dragging = false
                input.button(0, down: false, at: cursor.position)
            }
            magnifying = true
            magnifierCancelled = false
            surface?.setMagnifierVisible(true, at: location)
            movePointer(to: location)
        case .changed:
            guard magnifying else { return }
            let inside = surface?.bounds.contains(location) == true
            if !inside { magnifierCancelled = true }
            if !magnifierCancelled { movePointer(to: location) }
            surface?.setMagnifierVisible(!magnifierCancelled, at: location)
        case .ended, .cancelled:
            guard magnifying else { return }
            let inside = recognizer.state == .ended && surface?.bounds.contains(location) == true
            if inside && !magnifierCancelled {
                movePointer(to: location)
                input.click(0, at: cursor.position)
                impactFeedback()
            }
            magnifying = false
            surface?.setMagnifierVisible(false)
            surface?.finishViewportGesture()
            surface?.reportGesture("magnifier")
        default:
            break
        }
    }

    @objc private func onScroll(_ recognizer: UIPanGestureRecognizer) {
        guard canInteract, !pinching else { return }
        let translation = recognizer.translation(in: surface)
        recognizer.setTranslation(.zero, in: surface)
        switch recognizer.state {
        case .began:
            scrolling = true
            sendScroll(dx: translation.x * 2, dy: translation.y * 2, phase: 1)
        case .changed:
            if scrolling { sendScroll(dx: translation.x * 2, dy: translation.y * 2, phase: 2) }
        case .ended, .cancelled:
            guard scrolling else { return }
            scrolling = false
            let delta = recognizer.state == .ended ? translation : .zero
            sendScroll(dx: delta.x * 2, dy: delta.y * 2, phase: 3)
            surface?.reportGesture("scroll")
            if recognizer.state == .ended { startMomentum(velocity: recognizer.velocity(in: surface)) }
        default:
            break
        }
    }

    private func sendScroll(dx: CGFloat, dy: CGFloat, phase: UInt8 = 0, momentum: UInt8 = 0) {
        let delta = (surface?.prefs ?? SessionGesturePrefs()).scrollDelta(CGVector(dx: dx, dy: dy))
        input.scroll(dx: delta.dx, dy: delta.dy, phase: phase, momentum: momentum)
    }

    private func impactFeedback() {
        guard surface?.prefs.haptics == true else { return }
        feedback.impactOccurred()
        feedback.prepare()
    }

    private func movePointer(to point: CGPoint) {
        guard let surface else { return }
        let normalized = TouchTapMapper.normalized(viewPoint: point, viewport: cursor.viewport)
        let target = cursor.viewport.viewPoint(fromNormalized: normalized)
        let current = cursor.viewport.viewPoint(fromNormalized: cursor.position)
        let position = cursor.move(viewDelta: CGPoint(x: target.x - current.x, y: target.y - current.y),
                                   gain: 1, now: CACurrentMediaTime())
        input.pointer(position)
        surface.updateMagnifierCursor(position)
        surface.followCursorIfNeeded()
    }

    private func startMomentum(velocity: CGPoint) {
        stopMomentum()
        momentum.start(velocity: velocity)
        guard momentum.isActive else { return }
        let link = CADisplayLink(target: momentumTarget, selector: #selector(MomentumTarget.tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        lastMomentumTime = nil
        displayLink = link
        link.add(to: .main, forMode: .common)
    }

    private func stepMomentum(_ link: CADisplayLink) {
        guard canInteract else { stopMomentum(); return }
        let dt = lastMomentumTime.map { link.timestamp - $0 } ?? 1 / 60
        lastMomentumTime = link.timestamp
        let step = momentum.step(dt: dt)
        if step.phase != 0 { sendScroll(dx: step.dx, dy: step.dy, momentum: step.phase) }
        if step.finished {
            displayLink?.invalidate()
            displayLink = nil
            lastMomentumTime = nil
        }
    }

    private func stopMomentum() {
        if momentum.stop() { input.scroll(dx: 0, dy: 0, momentum: 3) }
        displayLink?.invalidate()
        displayLink = nil
        lastMomentumTime = nil
    }

    deinit { displayLink?.invalidate() }
}
