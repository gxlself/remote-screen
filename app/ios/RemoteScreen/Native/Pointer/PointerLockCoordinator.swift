import GameController
import UIKit

/// UIKit lifecycle and GameController callbacks are serialized on the main queue.
final class PointerLockCoordinator {
    static let shared = PointerLockCoordinator()

    private(set) weak var activeSurface: RemoteSessionSurface?
    private(set) var mouseConnected = !GCMouse.mice().isEmpty
    private(set) var escaped = false
    private(set) var systemLocked = false
    private(set) var wantsLock = false
    private(set) var route: PointerRoute = .idle

    private let surfaces = NSHashTable<RemoteSessionSurface>.weakObjects()
    private weak var requestedRoot: UIViewController?
    private var observers: [NSObjectProtocol] = []
    private var mouseInput: LockedMouseInput?
    private var lastState: State?

    private struct State: Equatable {
        let mouse: Bool
        let locked: Bool
        let reason: PointerLockReason
        let route: PointerRoute
    }

    private init() {
        precondition(Thread.isMainThread)
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: UIPointerLockState.didChangeNotification,
                                             object: nil, queue: .main) { [weak self] note in
            guard let self,
                  let scene = note.userInfo?[UIPointerLockState.sceneUserInfoKey] as? UIWindowScene,
                  scene === self.activeSurface?.window?.windowScene else { return }
            self.refresh()
        })
        for name in [Notification.Name.GCMouseDidConnect, .GCMouseDidDisconnect] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                // GCMouse.mice()/current settle on the next main run-loop turn.
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.mouseConnected = !GCMouse.mice().isEmpty
                    if name == .GCMouseDidDisconnect || !self.mouseConnected { self.escaped = false }
                    self.refresh()
                }
            })
        }
        mouseInput = LockedMouseInput(coordinator: self)
    }

    func surfaceDidChange(_ surface: RemoteSessionSurface) {
        precondition(Thread.isMainThread)
        surfaces.add(surface)
        let previous = activeSurface
        if surface.pointerVisible && surface.sessionInputReady {
            activeSurface = surface
        } else if activeSurface === surface {
            activeSurface = nil
        }
        if previous !== activeSurface {
            escaped = false
            previous?.cancelPointerInput()
        }
        refresh()
    }

    func escape() {
        precondition(Thread.isMainThread)
        guard activeSurface != nil, route == .relative else { return }
        escaped = true
        RsLog.marker("POINTER", ["escape": true])
        refresh()
    }

    func relock() {
        precondition(Thread.isMainThread)
        guard escaped, activeSurface != nil else { return }
        // UIKit and GC can both see the click that asks for a new lock. Drop
        // its GC down/up pair even if the system grants the lock mid-click.
        mouseInput?.suppressRelockClick()
        escaped = false
        RsLog.marker("POINTER", ["relock": true])
        refresh()
    }

    private func isFullScreen(_ surface: RemoteSessionSurface?) -> Bool {
        guard let window = surface?.window, let scene = window.windowScene else { return false }
        return PointerLockPolicy.isFullScreen(window: window.bounds.size, screen: scene.screen.bounds.size)
    }

    private func inputs(for surface: RemoteSessionSurface?) -> PointerLockInputs {
        let active = surface != nil && surface === activeSurface
        let visible = active && surface?.pointerVisible == true
        let streaming = surface?.sessionInputReady == true
        let fullScreen = isFullScreen(surface)
        // Stop accepting relative input immediately when eligibility is lost;
        // UIKit's acknowledgement of an unlock can arrive a run-loop later.
        let locked = systemLocked && active && mouseConnected && streaming && visible && fullScreen && !escaped
        return PointerLockInputs(mouseConnected: mouseConnected, streaming: streaming,
                                 visible: visible, fullScreen: fullScreen,
                                 escaped: active && escaped, systemLocked: locked)
    }

    private func refresh() {
        precondition(Thread.isMainThread)
        if !mouseConnected || !isFullScreen(activeSurface) || activeSurface?.pointerVisible != true {
            escaped = false
        }
        let wasSystemLocked = systemLocked
        systemLocked = activeSurface?.window?.windowScene?.pointerLockState?.isLocked ?? false
        let input = inputs(for: activeSurface)
        // P14A leaves fullScreen out of wantsLock; iPadSplit
        // must never request a lock, regardless of the OS's current policy.
        let nextWantsLock = input.fullScreen && PointerLockPolicy.wantsLock(input)
        let nextRoute: PointerRoute = input.mouseConnected && input.streaming && input.visible
            ? PointerLockPolicy.route(input) : .idle
        let next = State(mouse: mouseConnected, locked: input.systemLocked,
                         reason: PointerLockPolicy.reason(input), route: nextRoute)
        let oldRoute = route
        route = nextRoute
        let wantChanged = wantsLock != nextWantsLock
        wantsLock = nextWantsLock

        if (oldRoute == .relative && route != .relative) || (wasSystemLocked && !systemLocked) {
            activeSurface?.cancelPointerInput()
        }
        let root = activeSurface?.window?.rootViewController
        let oldRoot = requestedRoot
        requestedRoot = root
        if wantChanged { RsLog.marker("POINTER", ["want": wantsLock, "reason": next.reason.rawValue]) }
        if next != lastState {
            lastState = next
            RsLog.marker("POINTER", ["route": route.rawValue, "locked": next.locked,
                                     "mouse": mouseConnected, "reason": next.reason.rawValue])
        }
        for surface in surfaces.allObjects {
            let state = inputs(for: surface)
            surface.pointerRouteDidChange()
            surface.reportPointerLock(mouse: state.mouseConnected, locked: state.systemLocked,
                                      reason: PointerLockPolicy.reason(state))
        }
        // Publish first: UIKit may notify observers while updating the lock.
        if wantChanged || oldRoot !== root {
            oldRoot?.setNeedsUpdateOfPrefersPointerLocked()
            if root !== oldRoot { root?.setNeedsUpdateOfPrefersPointerLocked() }
        }
    }
}
