import UIKit
import UIKit.UIGestureRecognizerSubclass

private enum PencilStrokeEvent {
    case began(CGPoint, time: TimeInterval)
    case moved(CGPoint)
    case ended(CGPoint)
    case cancelled
}

/// Converts Apple Pencil touches into absolute pointer and left-button input.
final class PencilGestureController: NSObject, UIGestureRecognizerDelegate {
    private weak var surface: RemoteSessionSurface?
    private let input: SessionInputSender
    private let cursor: SessionCursorController
    private let hoverLayer: SessionPencilHoverLayer
    private var mapper = PencilStrokeMapper()
    private lazy var strokeRecognizer: PencilStrokeGestureRecognizer = {
        let recognizer = PencilStrokeGestureRecognizer(target: nil, action: nil)
        recognizer.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.pencil.rawValue)]
        recognizer.delaysTouchesBegan = false
        recognizer.cancelsTouchesInView = false
        recognizer.onEvent = { [weak self] event in self?.handleStroke(event) }
        return recognizer
    }()
    private lazy var hoverRecognizer: UIHoverGestureRecognizer = {
        let recognizer = UIHoverGestureRecognizer(target: self, action: #selector(onHover(_:)))
        recognizer.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.pencil.rawValue)]
        recognizer.cancelsTouchesInView = false
        return recognizer
    }()
    private lazy var recognizers: [UIGestureRecognizer] = {
        [strokeRecognizer, hoverRecognizer]
    }()
    private var installed = false
    private var suspended = false
    private var strokeDragged = false
    private var strokeClicks: UInt8 = 1

    init(surface: RemoteSessionSurface, input: SessionInputSender, cursor: SessionCursorController) {
        self.surface = surface
        self.input = input
        self.cursor = cursor
        self.hoverLayer = surface.pencilHoverLayer
        super.init()
        for recognizer in recognizers { recognizer.delegate = self }
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
        let actions = mapper.reset()
        apply(actions)
        surface?.pencilActive = false
        hoverLayer.hide()
        strokeDragged = false
        strokeClicks = 1
        mapper.hoverEnded()
        for recognizer in recognizers {
            recognizer.isEnabled = false
            recognizer.isEnabled = !suspended
        }
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldReceive touch: UITouch) -> Bool {
        canInteract
    }

    private var canInteract: Bool {
        installed && !suspended && surface?.sessionInputReady == true
    }

    private func handleStroke(_ event: PencilStrokeEvent) {
        guard let surface else { return }
        switch event {
        case let .began(point, time):
            guard canInteract else { return }
            surface.pencilActive = true
            hoverLayer.hide()
            mapper.hoverEnded()
            strokeDragged = false
            let actions = mapper.began(viewPoint: point, viewport: cursor.viewport, time: time)
            apply(actions)
            if let clicks = actions.compactMap(Self.downClicks).first {
                strokeClicks = clicks
                RsLog.marker("PENCIL", ["down": Int(clicks)])
            }
        case let .moved(point):
            guard canInteract else { return }
            let actions = mapper.moved(viewPoint: point, viewport: cursor.viewport)
            if actions.contains(where: Self.isMove) { strokeDragged = true }
            apply(actions)
        case let .ended(point):
            guard canInteract else {
                _ = mapper.reset()
                mapper.hoverEnded()
                surface.pencilActive = false
                hoverLayer.hide()
                return
            }
            let actions = mapper.ended(viewPoint: point, viewport: cursor.viewport)
            if actions.contains(where: Self.isMove) { strokeDragged = true }
            apply(actions)
            mapper.hoverEnded()
            surface.pencilActive = false
            hoverLayer.hide()
            let kind = strokeDragged ? "drag" : (strokeClicks >= 2 ? "doubleClick" : "click")
            surface.reportGesture(kind)
            strokeDragged = false
            strokeClicks = 1
        case .cancelled:
            let actions = mapper.cancelled()
            apply(actions)
            mapper.hoverEnded()
            surface.pencilActive = false
            hoverLayer.hide()
            strokeDragged = false
            strokeClicks = 1
        }
    }

    @objc private func onHover(_ recognizer: UIHoverGestureRecognizer) {
        guard let surface, canInteract else { return }
        let point = recognizer.location(in: surface)
        switch recognizer.state {
        case .began:
            guard !surface.pencilActive else { return }
            RsLog.marker("PENCIL", ["hover": "begin"])
            hoverLayer.show(at: point)
            apply(mapper.hover(viewPoint: point, viewport: cursor.viewport))
        case .changed:
            guard !surface.pencilActive else { return }
            hoverLayer.show(at: point)
            apply(mapper.hover(viewPoint: point, viewport: cursor.viewport))
        case .ended, .cancelled:
            mapper.hoverEnded()
            hoverLayer.hide()
            RsLog.marker("PENCIL", ["hover": "end"])
        default:
            break
        }
    }

    private func apply(_ actions: [PencilAction]) {
        for action in actions {
            switch action {
            case let .move(position):
                let now = CACurrentMediaTime()
                let current = cursor.moveTo(normalized: position, now: now)
                input.pointer(current)
                surface?.updateMagnifierCursor(current)
            case let .down(position, clicks):
                input.flushPointer()
                input.button(0, down: true, at: position, clicks: clicks)
            case let .up(position, clicks):
                input.flushPointer()
                input.button(0, down: false, at: position, clicks: clicks)
                RsLog.marker("PENCIL", ["up": Int(clicks), "drag": strokeDragged])
            }
        }
    }

    private static func isMove(_ action: PencilAction) -> Bool {
        if case .move = action { return true }
        return false
    }

    private static func downClicks(_ action: PencilAction) -> UInt8? {
        if case let .down(_, clicks) = action { return clicks }
        return nil
    }
}

private final class PencilStrokeGestureRecognizer: UIGestureRecognizer {
    var onEvent: ((PencilStrokeEvent) -> Void)?
    private var activeTouch: UITouch?

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        guard activeTouch == nil, let touch = touches.first, let view else {
            if activeTouch == nil { state = .failed }
            return
        }
        activeTouch = touch
        state = .began
        onEvent?(.began(touch.preciseLocation(in: view), time: touch.timestamp))
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let activeTouch, touches.contains(where: { $0 === activeTouch }), let view else { return }
        state = .changed
        onEvent?(.moved(activeTouch.preciseLocation(in: view)))
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let activeTouch, touches.contains(where: { $0 === activeTouch }), let view else { return }
        state = .ended
        onEvent?(.ended(activeTouch.preciseLocation(in: view)))
        self.activeTouch = nil
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let activeTouch, touches.contains(where: { $0 === activeTouch }) else { return }
        state = .cancelled
        onEvent?(.cancelled)
        self.activeTouch = nil
    }

    override func reset() {
        activeTouch = nil
        super.reset()
    }
}
