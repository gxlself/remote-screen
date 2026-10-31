import GameController
import UIKit

final class LockedMouseInput {
    // Verify both signs with Magic Keyboard and a Bluetooth mouse on device.
    private static let movementYSign: CGFloat = -1
    private static let logInterval: TimeInterval = 1

    private weak var coordinator: PointerLockCoordinator?
    private var mouse: GCMouse?
    private var observers: [NSObjectProtocol] = []
    private var suppressPrimary = false
    private var lastScrollLog: TimeInterval?

    init(coordinator: PointerLockCoordinator) {
        self.coordinator = coordinator
        for name in [Notification.Name.GCMouseDidBecomeCurrent, .GCMouseDidConnect, .GCMouseDidDisconnect] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil,
                                                                    queue: .main) { [weak self] _ in
                DispatchQueue.main.async { [weak self] in self?.bindCurrent() }
            })
        }
        bindCurrent()
    }

    func suppressRelockClick() {
        suppressPrimary = true
        // A very short tap can finish in GC before UIKit delivers its down.
        // Do not leave the next, unrelated click suppressed in that case.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            if self.mouse?.mouseInput?.leftButton.isPressed != true { self.suppressPrimary = false }
        }
    }

    private var surface: RemoteSessionSurface? {
        guard coordinator?.route == .relative else { return nil }
        return coordinator?.activeSurface
    }

    func bindCurrent() {
        precondition(Thread.isMainThread)
        // `current` can lag behind a reconnect; any attached mouse will do.
        let current = GCMouse.current ?? GCMouse.mice().first
        guard current !== mouse else { return }
        RsLog.marker("POINTER", ["gcBind": current != nil, "mice": GCMouse.mice().count])
        coordinator?.activeSurface?.cancelPointerInput()
        let wasSuppressing = suppressPrimary
        unbind()
        mouse = current
        guard let current, let input = current.mouseInput else { return }
        suppressPrimary = wasSuppressing && input.leftButton.isPressed
        current.handlerQueue = .main
        input.mouseMovedHandler = { [weak self, weak current] _, dx, dy in
            guard let self, let current, self.mouse === current else { return }
            self.surface?.pointerRelativeMove(raw: CGPoint(x: CGFloat(dx), y: CGFloat(dy) * Self.movementYSign),
                                              now: CACurrentMediaTime())
        }
        bind(input.leftButton, button: PointerButtons.left)
        bind(input.rightButton, button: PointerButtons.right)
        bind(input.middleButton, button: PointerButtons.middle)
        for (index, auxiliary) in (input.auxiliaryButtons ?? []).prefix(2).enumerated() {
            if let button = PointerButtons.auxiliary(index: index) { bind(auxiliary, button: button) }
        }
        input.scroll.valueChangedHandler = { [weak self, weak current] _, x, y in
            guard let self, let current, self.mouse === current else { return }
            let now = CACurrentMediaTime()
            let surface = self.surface
            if self.lastScrollLog.map({ now - $0 >= Self.logInterval }) ?? true {
                self.lastScrollLog = now
                let scene = self.coordinator?.activeSurface?.window?.windowScene
                RsLog.marker("POINTER", ["gcScroll": [Double(x), Double(y)], "locked": surface != nil,
                                         "sysLocked": scene?.pointerLockState?.isLocked == true])
            }
            // Magic Keyboard on iPadOS 26: GameController puts the trackpad's
            // up/down scroll on x, positive as the fingers move down, and
            // left/right on y. The surface takes UIKit's translation axes.
            surface?.pointerLockedScroll(x: CGFloat(y), y: CGFloat(x), now: now)
        }
    }

    private func bind(_ input: GCControllerButtonInput?, button: Int32) {
        guard let mouse else { return }
        input?.pressedChangedHandler = { [weak self, weak mouse] _, _, down in
            guard let self, let mouse, self.mouse === mouse else { return }
            if button == PointerButtons.left && self.suppressPrimary {
                if !down { self.suppressPrimary = false }
                return
            }
            self.surface?.pointerButton(button, down: down, now: CACurrentMediaTime())
        }
    }

    private func unbind() {
        if let input = mouse?.mouseInput {
            input.mouseMovedHandler = nil
            input.leftButton.pressedChangedHandler = nil
            input.rightButton?.pressedChangedHandler = nil
            input.middleButton?.pressedChangedHandler = nil
            for auxiliary in input.auxiliaryButtons ?? [] { auxiliary.pressedChangedHandler = nil }
            input.scroll.valueChangedHandler = nil
        }
        mouse = nil
        suppressPrimary = false
    }

    deinit {
        unbind()
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }
}
