import Foundation
import UIKit

@objc(RsLifecycleMonitor)
final class RsLifecycleMonitor: NSObject {
    typealias DictionaryHandler = (NSDictionary) -> Void

    @objc(shared)
    static let shared = RsLifecycleMonitor()

    private let marker = LifecycleExitMarker(storage: UserDefaults.standard)
    private let notificationCenter = NotificationCenter.default
    private var observers: [NSObjectProtocol] = []
    private var budgetWorkItem: DispatchWorkItem?
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var backgroundStartedAt: TimeInterval?
    private var powerHandler: DictionaryHandler?
    private var backgroundHandler: DictionaryHandler?
    private var currentPower: LifecyclePowerState
    private(set) var previousRunEndedInBackground = false
    private(set) var keepAlive = false
    private var started = false

    private override init() {
        currentPower = LifecyclePowerPolicy.state(
            lowPowerMode: ProcessInfo.processInfo.isLowPowerModeEnabled,
            level: -1,
            batteryState: .unknown
        )
        super.init()
    }

    @objc(start)
    func start() {
        if Thread.isMainThread {
            startOnMain()
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.startOnMain()
            }
        }
    }

    @objc(registerWithPowerChangeCallback:backgroundTaskCallback:)
    func register(
        withPowerChangeCallback powerChange: @escaping DictionaryHandler,
        backgroundTaskCallback background: @escaping DictionaryHandler
    ) {
        start()
        if Thread.isMainThread {
            powerHandler = powerChange
            backgroundHandler = background
        } else {
            DispatchQueue.main.async { [weak self] in
                self?.powerHandler = powerChange
                self?.backgroundHandler = background
            }
        }
    }

    @objc(constants)
    func constants() -> NSDictionary {
        ["previousRunEndedInBackground": previousRunEndedInBackground] as NSDictionary
    }

    @objc(powerState)
    func powerState() -> NSDictionary {
        if Thread.isMainThread {
            return dictionary(for: currentPower)
        }
        return DispatchQueue.main.sync { [weak self] in
            guard let self else {
                return NSDictionary()
            }
            return self.dictionary(for: self.currentPower)
        }
    }

    @objc(setIdleTimerDisabled:)
    func setIdleTimerDisabled(_ disabled: Bool) {
        performOnMain { [weak self] in
            guard self != nil else { return }
            UIApplication.shared.isIdleTimerDisabled = disabled
            RsLog.marker("LIFECYCLE", ["idleTimerDisabled": disabled])
        }
    }

    @objc(setBackgroundKeepAlive:)
    func setBackgroundKeepAlive(_ enabled: Bool) {
        performOnMain { [weak self] in
            guard let self else { return }
            self.keepAlive = enabled
            if !enabled {
                self.finishBackgroundTask(reason: "disabled")
            } else if UIApplication.shared.applicationState == .background {
                self.beginBackgroundTaskIfNeeded()
            }
        }
    }

    private func startOnMain() {
        guard !started else { return }
        started = true
        previousRunEndedInBackground = marker.start()

        let device = UIDevice.current
        device.isBatteryMonitoringEnabled = true
        currentPower = readPowerState()

        observe(UIApplication.didEnterBackgroundNotification) { [weak self] in
            self?.didEnterBackground()
        }
        observe(UIApplication.willEnterForegroundNotification) { [weak self] in
            self?.willEnterForeground()
        }
        observe(UIApplication.willTerminateNotification) { [weak self] in
            self?.willTerminate()
        }
        observe(
            Notification.Name("NSProcessInfoPowerStateDidChangeNotification")
        ) { [weak self] in
            self?.powerDidChange()
        }
        observe(
            Notification.Name("UIDeviceBatteryLevelDidChangeNotification")
        ) { [weak self] in
            self?.powerDidChange()
        }
        observe(
            Notification.Name("UIDeviceBatteryStateDidChangeNotification")
        ) { [weak self] in
            self?.powerDidChange()
        }
    }

    private func observe(_ name: Notification.Name, handler: @escaping () -> Void) {
        let token = notificationCenter.addObserver(
            forName: name,
            object: nil,
            queue: .main
        ) { _ in
            handler()
        }
        observers.append(token)
    }

    private func performOnMain(_ action: @escaping () -> Void) {
        if Thread.isMainThread {
            action()
        } else {
            DispatchQueue.main.async(execute: action)
        }
    }

    private func didEnterBackground() {
        marker.didEnterBackground()
        if keepAlive {
            beginBackgroundTaskIfNeeded()
        }
    }

    private func willEnterForeground() {
        marker.willEnterForeground()
        finishBackgroundTask(reason: "foreground")
    }

    private func willTerminate() {
        marker.willTerminate()
        finishBackgroundTask(reason: "terminated")
    }

    private func beginBackgroundTaskIfNeeded() {
        guard backgroundTask == .invalid else { return }
        let startedAt = ProcessInfo.processInfo.systemUptime
        backgroundStartedAt = startedAt
        let task = UIApplication.shared.beginBackgroundTask(
            withName: "rs.session.background"
        ) { [weak self] in
            self?.finishBackgroundTask(reason: "expired")
        }
        guard task != .invalid else {
            backgroundStartedAt = nil
            return
        }
        backgroundTask = task
        let remaining = UIApplication.shared.backgroundTimeRemaining
        let hold = LifecycleBackgroundBudget.holdUntil(remaining: remaining)
        RsLog.marker("LIFECYCLE", ["event": "bgBegin", "remainingS": remaining])
        backgroundHandler?([
            "event": "begin",
            "reason": "",
            "heldMs": 0.0,
        ] as NSDictionary)

        budgetWorkItem?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.finishBackgroundTask(reason: "budget")
        }
        budgetWorkItem = work
        DispatchQueue.main.asyncAfter(
            deadline: .now() + hold,
            execute: work
        )
    }

    private func finishBackgroundTask(reason: String) {
        guard backgroundTask != .invalid else { return }
        budgetWorkItem?.cancel()
        budgetWorkItem = nil
        let task = backgroundTask
        backgroundTask = .invalid
        UIApplication.shared.endBackgroundTask(task)
        let heldMs: Double
        if let startedAt = backgroundStartedAt {
            let elapsedMs = max(0, (ProcessInfo.processInfo.systemUptime - startedAt) * 1000)
            heldMs = min(elapsedMs, LifecycleBackgroundBudget.maximumHold * 1000)
        } else {
            heldMs = 0
        }
        backgroundStartedAt = nil
        RsLog.marker("LIFECYCLE", [
            "event": "bgEnd",
            "reason": reason,
            "heldMs": heldMs,
        ])
        backgroundHandler?([
            "event": "end",
            "reason": reason,
            "heldMs": heldMs,
        ] as NSDictionary)
    }

    private func powerDidChange() {
        let next = readPowerState()
        guard LifecyclePowerPolicy.changed(currentPower, next) else { return }
        currentPower = next
        powerHandler?(dictionary(for: next))
    }

    private func readPowerState() -> LifecyclePowerState {
        let device = UIDevice.current
        let batteryState: BatteryStateKind
        switch device.batteryState {
        case .unplugged:
            batteryState = .unplugged
        case .charging:
            batteryState = .charging
        case .full:
            batteryState = .full
        case .unknown:
            batteryState = .unknown
        @unknown default:
            batteryState = .unknown
        }
        return LifecyclePowerPolicy.state(
            lowPowerMode: ProcessInfo.processInfo.isLowPowerModeEnabled,
            level: Double(device.batteryLevel),
            batteryState: batteryState
        )
    }

    private func dictionary(for state: LifecyclePowerState) -> NSDictionary {
        [
            "lowPower": state.lowPower,
            "lowPowerMode": state.lowPowerMode,
            "batteryLevel": state.batteryLevel,
            "charging": state.charging,
        ] as NSDictionary
    }
}
