import Foundation

/// The fields shared by Fabric's 1 Hz event and the native 5 s log marker.
struct SessionVideoStats: Equatable {
    let fps: Int
    let decodeP50Ms: Double
    let decodeP95Ms: Double
    let latencyP50Ms: Double
    let queue: Int
    let dropped: Int
    let keyframeRequests: Int
    let width: Int
    let height: Int
    let codec: String

    var fields: [String: Any] {
        [
            "fps": fps,
            "decodeP50Ms": decodeP50Ms,
            "decodeP95Ms": decodeP95Ms,
            "latencyP50Ms": latencyP50Ms,
            "queue": queue,
            "dropped": dropped,
            "keyframeRequests": keyframeRequests,
            "width": width,
            "height": height,
            "codec": codec,
        ]
    }

    func dictionary() -> NSDictionary { fields as NSDictionary }
}

/// Session-scoped adapter for the per-frame decode report ABI. Calls happen on
/// the pipeline's serial decode queue after a frame is submitted to the
/// renderer.
final class SessionDecodeReporter {
    private let lock = NSLock()
    private var sessionId: UInt64?

    func setSessionId(_ id: UInt64?) {
        lock.lock()
        sessionId = id
        lock.unlock()
    }

    func report(decodeUs: UInt64, latencyUs: UInt64, queue: UInt32) {
        lock.lock()
        let id = sessionId
        lock.unlock()
        guard let id else { return }
        rs_session_report_decoded(id, decodeUs, latencyUs, queue)
    }
}
