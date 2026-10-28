import UIKit

/// Keeps a display preference alive only while a visible session has video.
/// AVSampleBufferDisplayLayer still presents every decoded frame immediately.
final class SessionFrameRateController {
    private final class DisplayLinkTarget: NSObject {
        @objc func tick(_ link: CADisplayLink) {}
    }

    private weak var surface: UIView?
    private weak var suspendedScene: UIScene?
    private let target = DisplayLinkTarget()
    private var displayLink: CADisplayLink?
    private var observers: [NSObjectProtocol] = []
    private var streaming = false
    private var appActive = UIApplication.shared.applicationState == .active
    private var appliedRange: DisplayFrameRatePolicy.Range?
    private var loggedRange = false

    init(surface: UIView) {
        self.surface = surface
        // Read P7's power state without replacing its JS callback.
        RsLifecycleMonitor.shared.start()
        observe(UIApplication.willResignActiveNotification) { [weak self] _ in
            self?.appActive = false
            self?.refresh()
        }
        observe(UIApplication.didEnterBackgroundNotification) { [weak self] _ in
            self?.appActive = false
            self?.refresh()
        }
        observe(UIApplication.didBecomeActiveNotification) { [weak self] _ in
            self?.appActive = true
            self?.refresh()
        }
        observe(UIScene.willDeactivateNotification) { [weak self] notification in
            guard let self, let scene = notification.object as? UIScene,
                  scene === self.surface?.window?.windowScene else { return }
            self.suspendedScene = scene
            self.refresh()
        }
        observe(UIScene.didActivateNotification) { [weak self] notification in
            guard let self, let scene = notification.object as? UIScene,
                  scene === self.surface?.window?.windowScene else { return }
            self.suspendedScene = nil
            self.refresh()
        }
        for name in [Notification.Name("NSProcessInfoPowerStateDidChangeNotification"),
                     Notification.Name("UIDeviceBatteryLevelDidChangeNotification"),
                     Notification.Name("UIDeviceBatteryStateDidChangeNotification")] {
            observe(name) { [weak self] _ in
                // Let P7 update its snapshot before reading it, regardless of
                // NotificationCenter's observer delivery order.
                DispatchQueue.main.async { [weak self] in self?.refresh() }
            }
        }
    }

    func setStreaming(_ streaming: Bool) {
        self.streaming = streaming
        refresh()
    }

    func updateVisibility() { refresh() }

    private func observe(_ name: Notification.Name, handler: @escaping (Notification) -> Void) {
        observers.append(NotificationCenter.default.addObserver(forName: name, object: nil,
                                                               queue: .main, using: handler))
    }

    private func refresh() {
        let window = surface?.window
        let scene = window?.windowScene
        let sceneActive = scene == nil || (scene?.activationState == .foregroundActive && scene !== suspendedScene)
        let visible = surface.map { isVisible($0, in: window) } ?? false
        let lowPower = (RsLifecycleMonitor.shared.powerState()["lowPower"] as? Bool)
            ?? ProcessInfo.processInfo.isLowPowerModeEnabled
        let range = DisplayFrameRatePolicy.range(
            maxFps: window?.screen.maximumFramesPerSecond ?? 60,
            lowPower: lowPower,
            streaming: streaming && appActive && sceneActive && visible
        )

        if let range {
            if displayLink == nil {
                let link = CADisplayLink(target: target, selector: #selector(DisplayLinkTarget.tick(_:)))
                displayLink = link
                link.add(to: .main, forMode: .common)
            }
            displayLink?.preferredFrameRateRange = CAFrameRateRange(
                minimum: Float(range.minimum), maximum: Float(range.maximum), preferred: Float(range.preferred)
            )
        } else {
            displayLink?.invalidate()
            displayLink = nil
        }
        if !loggedRange || range != appliedRange {
            loggedRange = true
            appliedRange = range
            // RsMarkerLine currently accepts scalar fields only. Keep this
            // required nested marker local instead of changing shared logging.
            if let range {
                NSLog("%@", "RS-SESSVIEW {\"frameRate\":{\"min\":\(range.minimum),\"max\":\(range.maximum)}}")
            } else {
                NSLog("%@", "RS-SESSVIEW {\"frameRate\":null}")
            }
        }
    }

    private func isVisible(_ view: UIView, in window: UIWindow?) -> Bool {
        guard let window, !window.isHidden, !view.bounds.isEmpty,
              view.convert(view.bounds, to: window).intersects(window.bounds) else { return false }
        var ancestor: UIView? = view
        while let current = ancestor {
            if current.isHidden || current.alpha <= 0.01 { return false }
            ancestor = current.superview
        }
        return true
    }

    deinit {
        displayLink?.invalidate()
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }
}
