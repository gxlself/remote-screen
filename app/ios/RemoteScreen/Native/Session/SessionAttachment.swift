import Foundation

/// Owned snapshots only: none of the C ABI's borrowed pointers escape a callback.
struct SessionFrame {
    let frameId: UInt32
    var epoch: UInt32
    let displayId: UInt8
    let keyframe: Bool
    let codec: UInt8
    let captureTimestampUs: UInt64
    let captureLocalUs: Int64
    let clockSynced: Bool
    let receivedUs: UInt64
    let vps: Data
    let sps: Data
    let pps: Data
    let avcc: Data

    init(copying frame: RsFrame) {
        frameId = frame.frame_id
        epoch = UInt32(frame.epoch)
        displayId = frame.display_id
        keyframe = frame.keyframe != 0
        codec = frame.codec
        captureTimestampUs = frame.capture_ts_us
        captureLocalUs = frame.capture_local_us
        clockSynced = frame.clock_synced != 0
        receivedUs = frame.received_us
        vps = sessionCopy(frame.vps, frame.vps_len)
        sps = sessionCopy(frame.sps, frame.sps_len)
        pps = sessionCopy(frame.pps, frame.pps_len)
        avcc = sessionCopy(frame.avcc, frame.avcc_len)
    }
}

struct SessionEvent {
    let kind: Int
    let width: UInt32
    let height: UInt32
    let fps: UInt32
    let displayId: UInt32
    let epoch: UInt32
    let codec: UInt8
    let cursorVisible: Bool
    let cursorX: Float
    let cursorY: Float
    let rttUs: UInt64
    let offsetUs: Int64
    let shapeId: UInt64
    let hotX: UInt32
    let hotY: UInt32
    let scale: Float
    let data: Data
    let message: String
    let srcX: Float
    let srcY: Float
    let srcW: Float
    let srcH: Float
    let displayW: UInt32
    let displayH: UInt32

    init(copying event: RsEvent) {
        kind = Int(event.kind)
        width = event.width
        height = event.height
        fps = event.fps
        displayId = event.display_id
        epoch = event.epoch
        codec = event.codec
        cursorVisible = event.cursor_visible != 0
        cursorX = event.cursor_x
        cursorY = event.cursor_y
        rttUs = event.rtt_us
        offsetUs = event.offset_us
        shapeId = event.shape_id
        hotX = event.hot_x
        hotY = event.hot_y
        scale = event.scale
        data = sessionCopy(event.data, event.data_len)
        message = event.message.map { String(cString: $0) } ?? ""
        srcX = event.src_x
        srcY = event.src_y
        srcW = event.src_w
        srcH = event.src_h
        displayW = event.display_w
        displayH = event.display_h
    }
}

/// P4A installs a sink on the surface. Events arrive on the main queue and own
/// their PNG bytes, including cached cursor replay during attachment.
protocol SessionCursorSink: AnyObject {
    func consume(_ event: SessionEvent)
}

final class SessionNullCursorSink: SessionCursorSink {
    func consume(_ event: SessionEvent) {}
}

private func sessionCopy(_ bytes: UnsafePointer<UInt8>?, _ count: Int) -> Data {
    guard count > 0, let bytes else { return Data() }
    return Data(bytes: bytes, count: count)
}

private final class SessionCallbackBox {
    let pipeline: SessionVideoPipeline

    init(pipeline: SessionVideoPipeline) { self.pipeline = pipeline }
}

// Noncapturing closures become C function pointers. Copy, submit, return; in
// particular, never dispatch synchronously to the UI or decode on this thread.
private let sessionFrameCallback: RsFrameCb = { context, frame in
    guard let context, let frame else { return }
    let box = Unmanaged<SessionCallbackBox>.fromOpaque(context).takeUnretainedValue()
    box.pipeline.submit(SessionFrame(copying: frame.pointee))
}

private let sessionEventCallback: RsEventCb = { context, event in
    guard let context, let event else { return }
    let box = Unmanaged<SessionCallbackBox>.fromOpaque(context).takeUnretainedValue()
    box.pipeline.submit(SessionEvent(copying: event.pointee))
}

/// Lifecycle methods run on the surface's main thread. detach is never called
/// from a Rust callback, so the ABI's return barrier makes releasing ctx safe.
final class SessionAttachment {
    private static let maxAttempts = 10
    private static let retryDelay: TimeInterval = 0.5
    private var attachedId: UInt64?
    private var context: UnsafeMutableRawPointer?
    private var puller: RawFramePuller?
    private var retry: DispatchWorkItem?
    private var generation: UInt64 = 0

    func attach(_ value: String, pipeline: SessionVideoPipeline) {
        detach()
        guard !value.isEmpty else { return }
        guard let id = SessionIdParse.parse(value) else {
            RsLog.marker("SESSVIEW", ["error": "bad-session-id"])
            return
        }
        attempt(id: id, value: value, pipeline: pipeline, number: 1, generation: generation)
    }

    func detach() {
        generation &+= 1
        retry?.cancel()
        retry = nil
        if let puller {
            puller.detach()
            self.puller = nil
        }
        if let id = attachedId { rs_session_detach(id) }
        attachedId = nil
        if let context {
            Unmanaged<SessionCallbackBox>.fromOpaque(context).release()
        }
        context = nil
    }

    private func attempt(id: UInt64, value: String, pipeline: SessionVideoPipeline,
                         number: Int, generation expected: UInt64) {
        guard expected == generation else { return }
        retry = nil
        // Mirrors RS_ADAPTER_ID_BASE from rs_mobile.h; the UInt64 shift also
        // works on toolchains that cannot import the C macro.
        let isRaw = id >= (UInt64(1) << 62)
        if isRaw {
            pipeline.willAttach(at: rs_now_us(), sessionId: id, raw: true)
            let rawPuller = RawFramePuller(pipeline: pipeline)
            if rawPuller.attach(id: id) {
                puller = rawPuller
                RsLog.marker("SESSVIEW", ["attach": true, "session": value, "raw": true])
                return
            }
            RsLog.marker("SESSVIEW", ["attach": false, "session": value, "raw": true])
            rawPuller.detach()
            guard number < Self.maxAttempts else { return }
            let work = DispatchWorkItem { [weak self, weak pipeline] in
                guard let self, let pipeline else { return }
                self.attempt(id: id, value: value, pipeline: pipeline,
                             number: number + 1, generation: expected)
            }
            retry = work
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.retryDelay, execute: work)
            return
        }

        let context = Unmanaged.passRetained(SessionCallbackBox(pipeline: pipeline)).toOpaque()
        pipeline.willAttach(at: rs_now_us(), sessionId: id)
        if rs_session_attach(id, context, sessionFrameCallback, sessionEventCallback) {
            attachedId = id
            self.context = context
            RsLog.marker("SESSVIEW", ["attach": true, "session": value])
            return
        }
        // A failed attachment retains no consumer or callback context.
        Unmanaged<SessionCallbackBox>.fromOpaque(context).release()
        RsLog.marker("SESSVIEW", ["attach": false, "session": value])
        guard number < Self.maxAttempts else { return }
        let work = DispatchWorkItem { [weak self, weak pipeline] in
            guard let self, let pipeline else { return }
            self.attempt(id: id, value: value, pipeline: pipeline,
                         number: number + 1, generation: expected)
        }
        retry = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.retryDelay, execute: work)
    }

    deinit { detach() }
}
