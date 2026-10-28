import Foundation

/// Calls the session-scoped keyframe request ABI. Gate actions already apply
/// the local 300 ms throttle and the 500 ms retry interval.
final class SessionKeyframeRequester {
    private let lock = NSLock()
    private var sessionId: UInt64?

    func setSessionId(_ id: UInt64?) {
        lock.lock()
        sessionId = id
        lock.unlock()
    }

    func request(reason: SessionKeyframeReason) {
        lock.lock()
        let id = sessionId
        lock.unlock()
        guard let id else { return }
        rs_session_request_keyframe(id, reason.wireValue)
        RsLog.marker("SESSVIEW", ["requestKeyframe": reason.wireName, "reason": Int(reason.wireValue)])
    }
}
