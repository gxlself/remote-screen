import Foundation

struct PipInputs: Equatable {
    var canStart: Bool
    var canContinue: Bool
    var surfaceReady: Bool

    static let none = PipInputs(canStart: false, canContinue: false, surfaceReady: false)
}

enum PipPhase: String, Equatable {
    case unsupported
    case idle
    case armed
    case starting
    case active
    case stopping
}

enum PipEvent: Equatable {
    case inputs(PipInputs)
    case possibleChanged(Bool)
    case userStart
    case userStop
    case willStart
    case didStart
    case failedToStart
    case willStop
    case didStop
    case restoreRequested
    case setPlaying(Bool)
    case appDidBecomeActive
    case appDidEnterBackground
}

enum PipAction: Equatable {
    case setAutoStart(Bool)
    case start
    case stop
    case setPassthrough(Bool)
    case invalidatePlaybackState
    case emitChange(active: Bool, reason: String)
    case emitPlayback(paused: Bool)
    case completeRestore(Bool)
}

/// Platform-neutral state machine for the iOS Picture in Picture adapter.
///
/// The adapter owns AVPictureInPictureController and interprets the actions
/// returned here. Keeping all transitions in this type makes lifecycle races
/// deterministic and testable without UIKit or AVFoundation.
struct PipMachine {
    private(set) var phase: PipPhase
    private(set) var paused = false
    private(set) var passthrough = false

    private var inputs = PipInputs.none
    private var possible = false
    private var appActive: Bool
    private var autoStart = false
    private var restoring = false
    private var userStopping = false
    private var wentBackgroundDuringPip = false
    private var holdPassthroughUntilActive = false
    private var stoppingReason: String?

    var isActive: Bool {
        phase == .starting || phase == .active
    }

    init(supported: Bool, appActive: Bool) {
        phase = supported ? .idle : .unsupported
        self.appActive = appActive
    }

    mutating func handle(_ event: PipEvent) -> [PipAction] {
        guard phase != .unsupported else { return [] }

        var actions: [PipAction] = []
        switch event {
        case .inputs(let value):
            inputs = value
            if (phase == .starting || phase == .active),
               (!value.canContinue || !value.surfaceReady) {
                phase = .stopping
                stoppingReason = value.surfaceReady ? "ineligible" : "detached"
                actions.append(.stop)
            } else if phase == .idle || phase == .armed {
                recalculateEligibility()
            }

        case .possibleChanged(let value):
            possible = value
            if phase == .idle || phase == .armed {
                recalculateEligibility()
            }

        case .userStart:
            guard phase == .armed, appActive else {
                actions.append(.emitChange(active: false, reason: "notPossible"))
                break
            }
            phase = .starting
            actions.append(.start)

        case .userStop:
            guard phase == .starting || phase == .active else { break }
            phase = .stopping
            userStopping = true
            actions.append(.stop)

        case .willStart:
            guard phase == .idle || phase == .armed || phase == .starting else { break }
            phase = .starting
            appendPassthrough(true, to: &actions)
            actions.append(.emitChange(active: true, reason: "willStart"))
            if !appActive {
                wentBackgroundDuringPip = true
            }

        case .didStart:
            phase = .active
            actions.append(.emitChange(active: true, reason: "started"))

        case .failedToStart:
            recalculateEligibility()
            if appActive {
                appendPassthrough(false, to: &actions)
            } else {
                holdPassthroughUntilActive = true
            }
            actions.append(.emitChange(active: false, reason: "failed"))

        case .willStop:
            guard phase == .starting || phase == .active else { break }
            phase = .stopping

        case .didStop:
            recalculateEligibility()
            let reason: String
            if restoring {
                reason = "restore"
            } else if userStopping {
                reason = "stopped"
            } else if let stoppingReason {
                reason = stoppingReason
            } else {
                reason = "closed"
            }
            actions.append(.emitChange(active: false, reason: reason))

            if paused {
                paused = false
                if appActive {
                    actions.append(.emitPlayback(paused: false))
                }
            }
            if appActive {
                appendPassthrough(false, to: &actions)
            } else {
                holdPassthroughUntilActive = true
            }

            restoring = false
            userStopping = false
            wentBackgroundDuringPip = false
            stoppingReason = nil

        case .restoreRequested:
            restoring = true
            actions.append(.completeRestore(inputs.surfaceReady))

        case .setPlaying(let playing):
            guard phase == .active else { break }
            let nextPaused = !playing
            guard paused != nextPaused else { break }
            paused = nextPaused
            actions.append(.emitPlayback(paused: paused))
            actions.append(.invalidatePlaybackState)

        case .appDidBecomeActive:
            appActive = true
            if phase == .active && wentBackgroundDuringPip && !restoring {
                phase = .stopping
                stoppingReason = "foreground"
                actions.append(.stop)
            }
            if holdPassthroughUntilActive,
               phase != .starting && phase != .active && phase != .stopping {
                holdPassthroughUntilActive = false
                appendPassthrough(false, to: &actions)
            }

        case .appDidEnterBackground:
            appActive = false
            if phase == .starting || phase == .active {
                wentBackgroundDuringPip = true
            }
        }

        appendAutoStartIfNeeded(to: &actions)
        return actions
    }

    private mutating func recalculateEligibility() {
        phase = inputs.canStart && inputs.surfaceReady && possible ? .armed : .idle
    }

    private mutating func appendPassthrough(_ enabled: Bool, to actions: inout [PipAction]) {
        guard passthrough != enabled else { return }
        passthrough = enabled
        actions.append(.setPassthrough(enabled))
    }

    private mutating func appendAutoStartIfNeeded(to actions: inout [PipAction]) {
        let wantAuto = (phase == .idle || phase == .armed)
            && inputs.canStart
            && inputs.surfaceReady
        guard wantAuto != autoStart else { return }
        autoStart = wantAuto
        actions.append(.setAutoStart(wantAuto))
    }
}
