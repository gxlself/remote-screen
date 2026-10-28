import Foundation

/// The platform independent state of the one audio output owned by the app.
public enum AudioPlaybackState: Equatable {
    case idle
    case running(String)
    case interrupted(String)
    case background(String)
    case failed(String, reason: String)

    public var name: String {
        switch self {
        case .idle: return "idle"
        case .running: return "running"
        case .interrupted: return "interrupted"
        case .background: return "background"
        case .failed: return "failed"
        }
    }

    public var session: String? {
        switch self {
        case .idle: return nil
        case .running(let value), .interrupted(let value), .background(let value): return value
        case .failed(let value, reason: _): return value
        }
    }
}

public enum AudioPlaybackEvent: Equatable {
    case start(String)
    case stop
    case interruptionBegan
    case interruptionEnded(shouldResume: Bool)
    case enterBackground
    case enterForeground
    case configurationChanged
    case mediaServicesReset
    case engineStartFailed(String)
}

public enum AudioPlaybackAction: Equatable {
    case activateSession
    case deactivateSession
    case openAudio(String)
    case closeAudio
    case startEngine
    case stopEngine
    case rebuildEngine
    case emit(state: AudioPlaybackState, reason: String)
}

/// Pure transition logic for the AVAudioEngine adapter.
///
/// The machine deliberately has no AVFoundation dependency. The app target
/// interprets its actions, which keeps interruption and lifecycle behavior
/// deterministic in NativeLogic tests.
public struct AudioPlaybackMachine {
    public private(set) var state: AudioPlaybackState
    private var inBackground: Bool
    private var interruptionActive = false

    public init(state: AudioPlaybackState = .idle) {
        self.state = state
        if case .background = state {
            inBackground = true
        } else {
            inBackground = false
        }
    }

    @discardableResult
    public mutating func handle(_ event: AudioPlaybackEvent) -> [AudioPlaybackAction] {
        switch event {
        case .start(let session):
            return start(session)
        case .stop:
            return stop()
        case .interruptionBegan:
            return interruptionBegan()
        case .interruptionEnded(let shouldResume):
            return interruptionEnded(shouldResume: shouldResume)
        case .enterBackground:
            return enterBackground()
        case .enterForeground:
            return enterForeground()
        case .configurationChanged:
            guard case .running = state else { return [] }
            return [.startEngine]
        case .mediaServicesReset:
            switch state {
            case .running:
                return [.rebuildEngine, .activateSession, .startEngine]
            case .idle, .interrupted, .background, .failed:
                return [.rebuildEngine]
            }
        case .engineStartFailed(let reason):
            guard let session = state.session else { return [] }
            state = .failed(session, reason: reason)
            return [.stopEngine, .closeAudio, .deactivateSession,
                    .emit(state: state, reason: reason)]
        }
    }

    private mutating func start(_ session: String) -> [AudioPlaybackAction] {
        guard !session.isEmpty else { return [] }

        let previous = state
        var needsOpen = previous.session != session
        if case .failed = previous { needsOpen = true }
        var actions: [AudioPlaybackAction] = []
        if needsOpen, previous != .idle {
            actions += [.stopEngine, .closeAudio]
        }

        if inBackground {
            state = .background(session)
        } else if interruptionActive {
            state = .interrupted(session)
        } else {
            state = .running(session)
            if previous != state { actions.append(.activateSession) }
        }
        if needsOpen { actions.append(.openAudio(session)) }
        if case .running = state { actions.append(.startEngine) }
        if previous != state {
            let reason = inBackground ? "background" : (interruptionActive ? "interruption" : "start")
            actions.append(.emit(state: state, reason: reason))
        }
        return actions
    }

    private mutating func stop() -> [AudioPlaybackAction] {
        guard state != .idle else { return [] }
        state = .idle
        return [.stopEngine, .closeAudio, .deactivateSession,
                .emit(state: state, reason: "stop")]
    }

    private mutating func interruptionBegan() -> [AudioPlaybackAction] {
        interruptionActive = true
        guard case .running(let session) = state else { return [] }
        state = .interrupted(session)
        return [.stopEngine, .emit(state: state, reason: "interruption")]
    }

    private mutating func interruptionEnded(shouldResume: Bool) -> [AudioPlaybackAction] {
        interruptionActive = false
        guard !inBackground else { return [] }
        guard case .interrupted(let session) = state, shouldResume else { return [] }
        state = .running(session)
        return [.activateSession, .startEngine,
                .emit(state: state, reason: "interruptionEnded")]
    }

    private mutating func enterBackground() -> [AudioPlaybackAction] {
        inBackground = true
        switch state {
        case .running(let session), .interrupted(let session):
            state = .background(session)
            return [.stopEngine, .deactivateSession,
                    .emit(state: state, reason: "background")]
        case .background, .idle, .failed:
            return []
        }
    }

    private mutating func enterForeground() -> [AudioPlaybackAction] {
        inBackground = false
        switch state {
        case .background(let session), .interrupted(let session):
            if interruptionActive {
                let next = AudioPlaybackState.interrupted(session)
                guard state != next else { return [] }
                state = next
                return [.emit(state: state, reason: "interruption")]
            }
            state = .running(session)
            return [.activateSession, .startEngine,
                    .emit(state: state, reason: "foreground")]
        case .idle, .running, .failed:
            return []
        }
    }
}
