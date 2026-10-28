import AVFoundation
import AVKit
import UIKit

/// Owns the platform PiP controller for the currently eligible session view.
/// UIKit and AVKit callbacks are delivered on the main queue; the active
/// mirror is protected separately because the JS sync method may be called
/// from any thread.
@objc(SessionPipCoordinator)
final class SessionPipCoordinator: NSObject {
    typealias DictionaryHandler = (NSDictionary) -> Void

    @objc static let shared = SessionPipCoordinator()

    private let supportedValue: Bool
    private var machine: PipMachine
    private let stateLock = NSLock()
    private var activeMirror = false

    private var observers: [NSObjectProtocol] = []
    private var changeHandler: DictionaryHandler?
    private var playbackHandler: DictionaryHandler?

    private var jsSessionId = ""
    private var requestedSessionId: UInt64?
    private var canStart = false
    private var canContinue = false
    private weak var surface: RemoteSessionSurface?
    private weak var latestSurface: RemoteSessionSurface?
    private let surfaces = NSHashTable<RemoteSessionSurface>.weakObjects()
    private var controller: SessionPipController?

    private var lastInputs: PipInputs?
    private var pendingPassthrough: Bool?
    private weak var appliedPassthroughPipeline: SessionVideoPipeline?
    private var appliedPassthrough: Bool?
    private var audioConfigured = false

    @objc var supported: Bool { supportedValue }

    private override init() {
        let supported = AVPictureInPictureController.isPictureInPictureSupported()
        supportedValue = supported
        machine = PipMachine(
            supported: supported,
            appActive: UIApplication.shared.applicationState == .active
        )
        super.init()

        guard supported else { return }
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: UIApplication.didBecomeActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.handle(.appDidBecomeActive)
        })
        observers.append(center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.handle(.appDidEnterBackground)
        })
    }

    deinit {
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    /// Called by every session surface lifecycle/layout hook. Only the view
    /// whose numeric session id matches the JS eligibility is a PiP source.
    func surfaceDidChange(_ candidate: RemoteSessionSurface) {
        guard supportedValue else { return }
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self] in
                self?.surfaceDidChange(candidate)
            }
            return
        }
        surfaces.add(candidate)
        latestSurface = candidate

        guard let requestedSessionId else {
            if surface === candidate {
                detachSurface()
            }
            return
        }

        guard candidate.sessionId == requestedSessionId else {
            if surface === candidate {
                detachSurface()
            }
            return
        }

        guard candidate.window != nil else {
            if surface === candidate {
                detachSurface()
            }
            return
        }

        let layerChanged = surface.map {
            $0.pipDisplayLayer !== candidate.pipDisplayLayer
        } ?? false
        if surface !== candidate || layerChanged || controller == nil {
            if surface != nil || controller != nil {
                detachSurface()
            }
            surface = candidate
            controller = SessionPipController(
                layer: candidate.pipDisplayLayer,
                isPaused: { [weak self] in
                    self?.machine.paused ?? false
                },
                sink: { [weak self] event in
                    self?.handle(event)
                }
            )
            appliedPassthroughPipeline = nil
            appliedPassthrough = nil
        } else {
            surface = candidate
        }

        applyPassthroughIfNeeded()
        refreshInputs()
    }

    @objc(setEligibility:canStart:canContinue:)
    func setEligibility(_ sessionId: String, canStart: Bool, canContinue: Bool) {
        guard supportedValue else { return }
        performOnMain { [weak self] in
            guard let self else { return }
            self.jsSessionId = sessionId
            self.requestedSessionId = UInt64(sessionId)
            self.canStart = canStart
            self.canContinue = canContinue
            self.configureAudioIfNeeded(canStart: canStart)

            if self.requestedSessionId == nil
                || self.surface?.sessionId != self.requestedSessionId
                || self.surface?.window == nil {
                self.detachSurface()
            }
            if self.surface == nil,
               let candidate = self.latestSurface,
               candidate.sessionId == self.requestedSessionId,
               candidate.window != nil {
                self.surfaceDidChange(candidate)
            } else if self.surface == nil,
                      let candidate = self.surfaces.allObjects.last(where: {
                          $0.sessionId == self.requestedSessionId && $0.window != nil
                      }) {
                self.surfaceDidChange(candidate)
            }
            self.refreshInputs()
        }
    }

    @objc func requestStart() {
        guard supportedValue else { return }
        performOnMain { [weak self] in self?.handle(.userStart) }
    }

    @objc func requestStop() {
        guard supportedValue else { return }
        performOnMain { [weak self] in self?.handle(.userStop) }
    }

    @objc func isActiveSync() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return supportedValue && activeMirror
    }

    @objc(registerWithChange:playback:)
    func register(change: @escaping DictionaryHandler, playback: @escaping DictionaryHandler) {
        guard supportedValue else { return }
        if Thread.isMainThread {
            changeHandler = change
            playbackHandler = playback
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.changeHandler = change
                self?.playbackHandler = playback
            }
        }
    }

    private func performOnMain(_ action: @escaping () -> Void) {
        if Thread.isMainThread {
            action()
        } else {
            DispatchQueue.main.async(execute: action)
        }
    }

    private func configureAudioIfNeeded(canStart: Bool) {
        guard supportedValue, canStart, !audioConfigured else { return }
        audioConfigured = true
        try? AVAudioSession.sharedInstance().setCategory(
            .playback,
            mode: .default,
            options: [.mixWithOthers]
        )
    }

    private func refreshInputs() {
        let next = PipInputs(
            canStart: canStart,
            canContinue: canContinue,
            surfaceReady: surface != nil && surface?.window != nil && surface?.sessionInputReady == true
        )
        guard next != lastInputs else { return }
        lastInputs = next
        handle(.inputs(next))
    }

    private func detachSurface() {
        surface = nil
        appliedPassthroughPipeline = nil
        appliedPassthrough = nil
        refreshInputs()
        let oldController = controller
        oldController?.tearDown()
        controller = nil
        if machine.phase == .stopping {
            handle(.didStop)
        } else {
            pendingPassthrough = machine.passthrough
        }
    }

    private func applyPassthroughIfNeeded() {
        guard let pipeline = surface?.pipPipeline else {
            pendingPassthrough = machine.passthrough
            return
        }
        let desired = pendingPassthrough ?? machine.passthrough
        guard pendingPassthrough != nil
            || appliedPassthroughPipeline !== pipeline
            || appliedPassthrough != desired else { return }
        pipeline.setPassthrough(desired)
        pendingPassthrough = nil
        appliedPassthroughPipeline = pipeline
        appliedPassthrough = desired
    }

    private func handle(_ event: PipEvent) {
        guard supportedValue else {
            updateActiveMirror(false)
            return
        }
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self] in
                self?.handle(event)
            }
            return
        }

        let oldPhase = machine.phase
        let actions = machine.handle(event)
        updateActiveMirror(machine.isActive)
        if oldPhase != machine.phase {
            RsLog.marker("PIP", [
                "phase": machine.phase.rawValue,
                "event": eventName(event),
                "reason": phaseReason(event: event, phase: machine.phase),
            ])
        }

        for action in actions {
            execute(action)
            updateActiveMirror(machine.isActive)
        }
    }

    private func execute(_ action: PipAction) {
        switch action {
        case .setAutoStart(let on):
            controller?.setAutoStart(on)
        case .start:
            controller?.start()
        case .stop:
            controller?.stop()
        case .setPassthrough(let enabled):
            if let pipeline = surface?.pipPipeline {
                pipeline.setPassthrough(enabled)
                pendingPassthrough = nil
                appliedPassthroughPipeline = pipeline
                appliedPassthrough = enabled
            } else {
                pendingPassthrough = enabled
                appliedPassthroughPipeline = nil
                appliedPassthrough = nil
            }
        case .invalidatePlaybackState:
            controller?.invalidatePlaybackState()
        case .emitChange(let active, let reason):
            RsLog.marker("PIP", [
                "phase": machine.phase.rawValue,
                "event": "emitChange",
                "reason": reason,
            ])
            changeHandler?([
                "sessionId": jsSessionId,
                "active": active,
                "reason": reason,
            ] as NSDictionary)
        case .emitPlayback(let paused):
            RsLog.marker("PIP", [
                "phase": machine.phase.rawValue,
                "event": "emitPlayback",
                "reason": paused ? "paused" : "playing",
            ])
            playbackHandler?([
                "sessionId": jsSessionId,
                "paused": paused,
            ] as NSDictionary)
        case .completeRestore(let ok):
            controller?.completeRestore(ok)
        }
    }

    private func updateActiveMirror(_ value: Bool) {
        stateLock.lock()
        activeMirror = value
        stateLock.unlock()
    }

    private func eventName(_ event: PipEvent) -> String {
        switch event {
        case .inputs: return "inputs"
        case .possibleChanged: return "possible"
        case .userStart: return "userStart"
        case .userStop: return "userStop"
        case .willStart: return "willStart"
        case .didStart: return "started"
        case .failedToStart: return "failed"
        case .willStop: return "willStop"
        case .didStop: return "stopped"
        case .restoreRequested: return "restore"
        case .setPlaying: return "setPlaying"
        case .appDidBecomeActive: return "foreground"
        case .appDidEnterBackground: return "background"
        }
    }

    private func phaseReason(event: PipEvent, phase: PipPhase) -> String {
        switch event {
        case .inputs(let value) where phase == .stopping:
            return value.surfaceReady ? "ineligible" : "detached"
        case .appDidBecomeActive where phase == .stopping:
            return "foreground"
        default:
            return phase.rawValue
        }
    }
}
