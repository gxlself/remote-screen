import Foundation

/// Events emitted by the Network.framework local-network authorization probe.
/// The probe's platform-independent terminal-state reducer.
///
/// A terminal event wins permanently. This matters when a timeout races with a
/// browser callback arriving on another queue: the first result is the one
/// returned to all callers.
struct LocalNetworkProbeState {
    enum Event: Equatable {
        case foundSelf
        case foundOther
        case waiting(Int)
        case listenerFailed
        case timeout
    }

    private(set) var result: String?

    init() {
        result = nil
    }

    @discardableResult
    mutating func receive(_ event: Event) -> String? {
        guard result == nil else { return result }
        switch event {
        case .foundSelf:
            result = "granted"
        case .waiting(let code) where code == -65570:
            result = "denied"
        case .listenerFailed, .timeout:
            result = "unknown"
        case .foundOther, .waiting:
            break
        }
        return result
    }
}
