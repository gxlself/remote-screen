import UIKit
import UIKit.UIGestureRecognizerSubclass

/// Indirect pointer input stays installed in both finger interaction modes.
final class PointerGestureController: NSObject, UIGestureRecognizerDelegate, UIPointerInteractionDelegate {
    // Match finger scrolling; confirm the multiplier on physical trackpads.
    private static let scrollMultiplier: CGFloat = 2

    private final class MomentumTarget: NSObject {
        weak var controller: PointerGestureController?
        @objc func tick(_ link: CADisplayLink) { controller?.stepMomentum(link) }
    }

    private weak var surface: RemoteSessionSurface?
    private let input: SessionInputSender
    private let cursor: SessionCursorController
    private var recognizers: [UIGestureRecognizer] = []
    private var interaction: UIPointerInteraction?
    private var installed = false
    private var suspended = false
    private var scrolling = false
    private var lastRoute: PointerRoute = .idle
    private var momentum = TrackpadScrollMomentum()
    private let momentumTarget = MomentumTarget()
    private var displayLink: CADisplayLink?
    private var lastMomentumTime: CFTimeInterval?
    private var lastDropLog: CFTimeInterval?
    private var lastRelativeScrollLog: CFTimeInterval?

    init(surface: RemoteSessionSurface, input: SessionInputSender, cursor: SessionCursorController) {
        self.surface = surface
        self.input = input
        self.cursor = cursor
        super.init()
        momentumTarget.controller = self

        let hover = UIHoverGestureRecognizer(target: self, action: #selector(onHover(_:)))
        hover.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.indirectPointer.rawValue)]
        let buttons = PointerButtonGestureRecognizer(target: nil, action: nil)
        buttons.allowedTouchTypes = hover.allowedTouchTypes
        buttons.onDown = { [weak self] button, point in
            guard let self, self.canInteract else { return false }
            if self.route == .idle, button == PointerButtons.left {
                PointerLockCoordinator.shared.relock()
                return false
            }
            guard self.route == .absolute else { return false }
            self.stopMomentum()
            self.move(to: point)
            self.surface?.pointerButton(button, down: true, now: CACurrentMediaTime())
            return true
        }
        buttons.onMove = { [weak self] point in
            guard let self, self.canInteract, self.route == .absolute else { return }
            self.move(to: point)
        }
        buttons.onUp = { [weak self] button in
            guard let self else { return }
            self.input.flushPointer()
            self.surface?.pointerButton(button, down: false, now: CACurrentMediaTime())
        }
        let continuous = UIPanGestureRecognizer(target: self, action: #selector(onContinuousScroll(_:)))
        continuous.allowedTouchTypes = []
        continuous.allowedScrollTypesMask = .continuous
        let discrete = UIPanGestureRecognizer(target: self, action: #selector(onDiscreteScroll(_:)))
        discrete.allowedTouchTypes = []
        discrete.allowedScrollTypesMask = .discrete
        recognizers = [hover, buttons, continuous, discrete]
        for recognizer in recognizers {
            recognizer.delegate = self
            recognizer.cancelsTouchesInView = false
        }
        interaction = UIPointerInteraction(delegate: self)
    }

    func install() {
        guard !installed, let surface else { return }
        installed = true
        for recognizer in recognizers {
            recognizer.isEnabled = !suspended
            surface.addGestureRecognizer(recognizer)
        }
        if let interaction { surface.addInteraction(interaction) }
        routeDidChange()
    }

    func uninstall() {
        cancel()
        guard installed else { return }
        installed = false
        for recognizer in recognizers { surface?.removeGestureRecognizer(recognizer) }
        if let interaction { surface?.removeInteraction(interaction) }
    }

    func setSuspended(_ suspended: Bool) {
        self.suspended = suspended
        if suspended { cancel() }
        for recognizer in recognizers { recognizer.isEnabled = !suspended }
        interaction?.invalidate()
    }

    func cancel() {
        if scrolling { input.scroll(dx: 0, dy: 0, phase: 3) }
        scrolling = false
        stopMomentum()
        for recognizer in recognizers {
            // UIKit may defer reset() until the next event cycle. Release a
            // held button now, before detach switches the input session.
            (recognizer as? PointerButtonGestureRecognizer)?.releaseButton()
            recognizer.isEnabled = false
            recognizer.isEnabled = !suspended
        }
    }

    func routeDidChange() {
        let next = route
        guard next != lastRoute else { return }
        // Leaving absolute input must end UIKit drags/scrolls. Entering it
        // from escaped must not reset the recognizer inside touchesBegan.
        if lastRoute == .absolute { cancel() }
        lastRoute = next
        interaction?.invalidate()
    }

    private var canInteract: Bool {
        installed && !suspended && surface?.sessionInputReady == true && surface?.pointerVisible == true
            && PointerLockCoordinator.shared.activeSurface === surface
    }

    private var route: PointerRoute {
        canInteract ? PointerLockCoordinator.shared.route : .idle
    }

    private func logDroppedClick() {
        let now = CACurrentMediaTime()
        guard lastDropLog.map({ now - $0 >= 1 }) ?? true else { return }
        lastDropLog = now
        RsLog.marker("POINTER", ["drop": "click", "installed": installed, "suspended": suspended,
                                 "ready": surface?.sessionInputReady == true,
                                 "visible": surface?.pointerVisible == true,
                                 "active": PointerLockCoordinator.shared.activeSurface === surface])
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool { true }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive event: UIEvent) -> Bool {
        guard canInteract else {
            if gestureRecognizer is PointerButtonGestureRecognizer, event.type == .touches { logDroppedClick() }
            return false
        }
        if gestureRecognizer is PointerButtonGestureRecognizer { return route != .relative }
        if gestureRecognizer is UIPanGestureRecognizer { return route != .idle }
        return route == .absolute
    }

    /// UIKit sends no trackpad scrolls while the pointer is locked; one in the
    /// relative route means the system let go of the lock without saying so,
    /// and GameController sends no scrolls then. Carry it until GameController
    /// scrolls again.
    private func takesScroll() -> Bool {
        switch route {
        case .absolute:
            return true
        case .relative:
            guard surface?.lockedScrollQuiet == true else { return false }
            let now = CACurrentMediaTime()
            if lastRelativeScrollLog.map({ now - $0 >= 1 }) ?? true {
                lastRelativeScrollLog = now
                RsLog.marker("POINTER", ["uiScroll": "relative",
                                         "sysLocked": surface?.window?.windowScene?.pointerLockState?.isLocked == true])
            }
            return true
        case .idle:
            return false
        }
    }

    /// GameController scrolls again: end a scroll carried in the relative
    /// route so the two never overlap.
    func yieldScroll() {
        guard route == .relative, scrolling || displayLink != nil else { return }
        if scrolling { input.scroll(dx: 0, dy: 0, phase: 3) }
        scrolling = false
        stopMomentum()
    }

    func pointerInteraction(_ interaction: UIPointerInteraction, styleFor region: UIPointerRegion) -> UIPointerStyle? {
        route == .absolute ? UIPointerStyle.hidden() : nil
    }

    @objc private func onHover(_ recognizer: UIHoverGestureRecognizer) {
        guard canInteract, route == .absolute else { return }
        if recognizer.state == .began || recognizer.state == .changed {
            move(to: recognizer.location(in: surface))
        } else if recognizer.state == .ended || recognizer.state == .cancelled {
            input.flushPointer()
        }
    }

    private func move(to point: CGPoint) {
        let normalized = TouchTapMapper.normalized(viewPoint: point, viewport: cursor.viewport)
        let position = cursor.moveTo(normalized: normalized, now: CACurrentMediaTime())
        input.pointer(position)
        surface?.updateMagnifierCursor(position)
    }

    @objc private func onContinuousScroll(_ recognizer: UIPanGestureRecognizer) {
        guard canInteract, route != .idle else { return }
        let delta = recognizer.translation(in: surface)
        recognizer.setTranslation(.zero, in: surface)
        switch recognizer.state {
        case .began:
            guard takesScroll() else { return }
            stopMomentum()
            scrolling = true
            sendScroll(delta, phase: 1)
        case .changed:
            if scrolling { sendScroll(delta, phase: 2) }
        case .ended, .cancelled:
            guard scrolling else { return }
            scrolling = false
            sendScroll(recognizer.state == .ended ? delta : .zero, phase: 3)
            if recognizer.state == .ended { startMomentum(velocity: recognizer.velocity(in: surface)) }
        default:
            break
        }
    }

    @objc private func onDiscreteScroll(_ recognizer: UIPanGestureRecognizer) {
        guard canInteract, takesScroll() else { return }
        let delta = recognizer.translation(in: surface)
        recognizer.setTranslation(.zero, in: surface)
        // A single wheel notch can be delivered with began/ended and no
        // changed callback. Preserve those deltas too, always with phase 0.
        if recognizer.state == .began || recognizer.state == .changed || recognizer.state == .ended {
            stopMomentum()
            if delta != .zero { sendScroll(delta) }
        }
    }

    private func sendScroll(_ delta: CGPoint, phase: UInt8 = 0) {
        sendScrollPixels(CGVector(dx: delta.x * Self.scrollMultiplier, dy: delta.y * Self.scrollMultiplier), phase: phase)
    }

    private func sendScrollPixels(_ delta: CGVector, phase: UInt8 = 0, momentum: UInt8 = 0) {
        let scaled = (surface?.prefs ?? SessionGesturePrefs()).scrollDelta(delta)
        input.scroll(dx: scaled.dx, dy: scaled.dy, phase: phase, momentum: momentum)
    }

    private func startMomentum(velocity: CGPoint) {
        stopMomentum()
        momentum.start(velocity: velocity)
        guard momentum.isActive else { return }
        let link = CADisplayLink(target: momentumTarget, selector: #selector(MomentumTarget.tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        displayLink = link
        link.add(to: .main, forMode: .common)
    }

    private func stepMomentum(_ link: CADisplayLink) {
        guard canInteract, route != .idle else { stopMomentum(); return }
        let dt = lastMomentumTime.map { link.timestamp - $0 } ?? 1 / 60
        lastMomentumTime = link.timestamp
        let step = momentum.step(dt: dt)
        // TrackpadScrollMomentum already applies the x2 pixel multiplier.
        if step.phase != 0 { sendScrollPixels(CGVector(dx: step.dx, dy: step.dy), momentum: step.phase) }
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

/// Recognize on button-down so a press-and-drag is sent without a tap delay.
private final class PointerButtonGestureRecognizer: UIGestureRecognizer {
    var onDown: ((Int32, CGPoint) -> Bool)?
    var onMove: ((CGPoint) -> Void)?
    var onUp: ((Int32) -> Void)?
    private var pressedButton: Int32?

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        guard pressedButton == nil, let touch = touches.first,
              let button = PointerButtons.fromButtonMask(Int(event.buttonMask.rawValue)),
              onDown?(button, touch.location(in: view)) == true else {
            state = .failed
            return
        }
        pressedButton = button
        state = .began
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard pressedButton != nil, let touch = touches.first else { return }
        onMove?(touch.location(in: view))
        state = .changed
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        if pressedButton != nil, let touch = touches.first { onMove?(touch.location(in: view)) }
        releaseButton()
        state = .ended
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        releaseButton()
        state = .cancelled
    }

    func releaseButton() {
        guard let button = pressedButton else { return }
        pressedButton = nil
        onUp?(button)
    }

    override func reset() {
        releaseButton()
        super.reset()
    }
}
