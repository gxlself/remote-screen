import AVFoundation
import CoreMedia
import Foundation
import QuartzCore
import os

/// Decoded pixels cover src in normalized, full-display coordinates.
struct EpochGeometry {
    let epoch: UInt32
    let src: CGRect
    let decoded: CGSize

    init(epoch: UInt32, src: CGRect, decoded: CGSize) {
        self.epoch = epoch
        self.src = src
        self.decoded = decoded
    }

    init(_ event: SessionEvent) {
        epoch = event.epoch
        src = event.displayW == 0 ? CGRect(x: 0, y: 0, width: 1, height: 1)
            : ViewportGeometry.source(x: event.srcX, y: event.srcY, w: event.srcW, h: event.srcH)
        decoded = CGSize(width: Int(event.width), height: Int(event.height))
    }
}

/// The ingress lock protects the gate and its bounded payload queue. Decoding,
/// ordinary frame submission and timers run on one serial queue. Geometry
/// changes hand one sample to the main queue and pause further submission;
/// Rust callbacks never wait for decoding or the UI.
final class SessionVideoPipeline {
    var onFirstFrame: ((UInt32, Int, Int) -> Void)?
    var onVideoSize: ((CGSize) -> Void)?
    var onEpochGeometry: ((EpochGeometry, CMSampleBuffer) -> Void)?
    var onVideoStats: ((NSDictionary) -> Void)?
    var onCursor: ((SessionEvent) -> Void)?
    var onClosed: (() -> Void)?
    var onDecode: ((SessionFrame, Double, Bool) -> Void)?
    private(set) var lastDecodeMs: Double = 0

    private let queue = DispatchQueue(label: "ai.gxlself.remote.session-video", qos: .userInteractive)
    private let lock = NSLock()
    private let decoder = SessionVideoDecoder()
    private let renderer: SessionFrameRenderer
    private let requester = SessionKeyframeRequester()
    private let reporter = SessionDecodeReporter()
    private let stats = SessionDecodeStats()
    private let logger = Logger(subsystem: "ai.gxlself.remotescreen", category: "SessionView")

    // Ingress-lock state.
    private var gate = SessionFrameGate()
    private var pending: [SessionFrame] = []
    private var epochs = SessionEpochTracker()
    private var configEpoch: UInt32?
    private var pendingEpoch: UInt32?
    private var accepting = false
    private var generation: UInt64 = 0
    private var attachedAtUs: UInt64 = 0
    private var drainScheduled = false
    private var timerScheduled = false
    private var dropped = 0
    private var keyframeRequests = 0
    private var lastKeyframeRequestAt: TimeInterval?
    private var latestFrameBuffer: CVPixelBuffer?
    private var displayedGeometry: EpochGeometry?
    private var geometryPresentationPending = false
    private var passthroughRequested = false
    private var rawMode = false

    /// A retained reference to the newest decoded image for local overlays
    /// such as the magnifier. This is read-only to consumers and never affects
    /// the display-layer queue.
    func latestPixelBuffer() -> CVPixelBuffer? {
        lock.lock()
        let buffer = latestFrameBuffer
        lock.unlock()
        return buffer
    }

    // Decode-queue state.
    private var decoderEpoch: UInt32?
    private var epochGeometry: EpochGeometry?
    private var firstFramePending = true
    private var videoSize: CGSize = .zero
    private var codec = "hevc"
    private var lastFrame: (id: UInt32, epoch: UInt32)?
    private var renderSwitch = PipRenderSwitch()
    private var timer: DispatchSourceTimer?
    private var nextStatsAt: TimeInterval = 0
    private var nextLogAt: TimeInterval = 0

    init(layer: AVSampleBufferDisplayLayer) {
        renderer = SessionFrameRenderer(layer: layer)
    }

    func willAttach(at time: UInt64, sessionId: UInt64, raw: Bool = false) {
        lock.lock()
        generation &+= 1
        let expected = generation
        accepting = true
        attachedAtUs = time
        rawMode = raw
        if raw {
            requester.setSessionId(nil)
            reporter.setSessionId(nil)
        } else {
            requester.setSessionId(sessionId)
            reporter.setSessionId(sessionId)
        }
        pending.removeAll(keepingCapacity: true)
        gate = SessionFrameGate()
        gate.decoderReady(false)
        epochs = SessionEpochTracker()
        configEpoch = nil
        pendingEpoch = nil
        drainScheduled = false
        timerScheduled = false
        dropped = 0
        keyframeRequests = 0
        lastKeyframeRequestAt = nil
        latestFrameBuffer = nil
        displayedGeometry = nil
        geometryPresentationPending = false
        // Queue before rs_session_attach can replay its cached events. No
        // synchronous dispatch from the Rust callback or the main thread.
        queue.async { [self] in
            guard isActive(expected) else { return }
            cancelTimer()
            renderSwitch.resetForAttach()
            decoder.invalidate()
            decoderEpoch = nil
            epochGeometry = nil
            firstFramePending = true
            videoSize = .zero
            codec = raw ? "raw" : "hevc"
            lastFrame = nil
            stats.reset()
            lock.lock()
            let requested = passthroughRequested
            lock.unlock()
            if !raw { applyRenderMode(requested ? .passthrough : .decode, generation: expected) }
        }
        lock.unlock()
    }

    /// Accepts a copied adapter frame from RawFramePuller. The image is
    /// already BGRA and never enters the VideoToolbox decoder.
    func submitRaw(image: CVPixelBuffer, copyMs: Double) {
        lock.lock()
        guard accepting, rawMode, let epoch = configEpoch else {
            lock.unlock()
            return
        }
        let expected = generation
        lock.unlock()
        queue.async { [self] in
            guard isCurrent(epoch: epoch, generation: expected) else { return }
            prepareDecoder(epoch: epoch)
            guard isCurrent(epoch: epoch, generation: expected) else { return }
            let size = CGSize(width: CVPixelBufferGetWidth(image), height: CVPixelBufferGetHeight(image))
            // A resize can deliver its config after the old-size copy finished
            // but before ingress. Do not label that buffer with the new epoch.
            guard epochGeometry?.decoded == size else { return }
            let decoded = SessionDecodedFrame(image: image,
                                               presentationTime: CMClockGetTime(CMClockGetHostTimeClock()),
                                               decodeMs: copyMs)
            presentDecoded(decoded, epoch: epoch, frame: nil, generation: expected)
        }
    }

    func submit(_ input: SessionFrame) {
        lock.lock()
        guard accepting else { lock.unlock(); return }
        let expected = generation
        let now = CACurrentMediaTime()
        var frame = input
        if let configEpoch {
            // RsFrame has the wire epoch byte, StreamConfig the full u32.
            guard UInt8(truncatingIfNeeded: configEpoch) == UInt8(truncatingIfNeeded: frame.epoch) else {
                dropped += 1
                lock.unlock()
                return
            }
            frame.epoch = configEpoch
        }
        observeEpochLocked(frame.epoch, now: now)
        pending.append(frame)
        let hasSets = SessionCodec.detect(vpsLen: frame.vps.count, spsLen: frame.sps.count,
                                         ppsLen: frame.pps.count) != .noParameterSets
        let actions = gate.push(SessionFrameMeta(frameId: frame.frameId, epoch: frame.epoch,
                                                 keyframe: frame.keyframe, hasParameterSets: hasSets),
                                now: now)
        let reasons = applyLocked(actions)
        startTimerLocked(generation: expected)
        scheduleDrainLocked(generation: expected)
        lock.unlock()
        request(reasons, generation: expected)
    }

    func submit(_ event: SessionEvent) {
        lock.lock()
        guard accepting else { lock.unlock(); return }
        let expected = generation
        if event.kind == RS_EV_STREAM_CONFIG {
            configEpoch = event.epoch
            observeEpochLocked(event.epoch, now: CACurrentMediaTime())
        } else if event.kind == RS_EV_CLOSED {
            accepting = false
            pending.removeAll(keepingCapacity: true)
            gate.reset(waitForKeyframe: true)
            requester.setSessionId(nil)
            reporter.setSessionId(nil)
        }
        // Order config/closure ahead of the next frame's drain scheduling.
        queue.async { [self] in handle(event, generation: expected) }
        if accepting { startTimerLocked(generation: expected) }
        lock.unlock()
    }

    func stop() {
        lock.lock()
        accepting = false
        generation &+= 1
        rawMode = false
        pending.removeAll(keepingCapacity: true)
        gate.reset(waitForKeyframe: true)
        latestFrameBuffer = nil
        displayedGeometry = nil
        geometryPresentationPending = false
        lock.unlock()
        requester.setSessionId(nil)
        reporter.setSessionId(nil)
        queue.async { [self] in cancelTimer(); decoder.invalidate() }
    }

    /// M3 V3：画中画期间改为直通，即压缩帧直接交给显示层解码。
    /// 任意线程调用；请求在入口锁里记录，切换在解码队列上进行。
    func setPassthrough(_ enabled: Bool) {
        lock.lock()
        passthroughRequested = enabled
        let expected = generation
        let active = accepting
        let raw = rawMode
        lock.unlock()
        guard active, !raw else { return }
        queue.async { [self] in
            guard isActive(expected) else { return }
            applyRenderMode(enabled ? .passthrough : .decode, generation: expected)
        }
    }

    /// The latest requested mode. The decode queue may still be applying the
    /// transition when this is read.
    var isPassthrough: Bool {
        lock.lock()
        let requested = passthroughRequested
        lock.unlock()
        return requested
    }

    /// Called under the ingress lock, including cached StreamConfig replay.
    private func observeEpochLocked(_ epoch: UInt32, now: TimeInterval) {
        let change = epochs.observe(epoch)
        pendingEpoch = epoch
        guard case .changed = change else { return }
        dropped += pending.count
        pending.removeAll(keepingCapacity: true)
        gate.reset(waitForKeyframe: true, now: now)
        gate.decoderReady(false)
    }

    private func applyLocked(_ actions: [SessionGateAction]) -> [SessionKeyframeReason] {
        var reasons: [SessionKeyframeReason] = []
        for action in actions {
            switch action {
            case .decode:
                break // next() owns removal from the metadata queue.
            case let .drop(id):
                let before = pending.count
                pending.removeAll { $0.frameId == id }
                dropped += before - pending.count
            case let .requestKeyframe(reason):
                reasons.append(reason)
            }
        }
        return reasons
    }

    private func request(_ reasons: [SessionKeyframeReason], generation expected: UInt64) {
        for reason in reasons {
            lock.lock()
            guard accepting, generation == expected else {
                lock.unlock()
                return
            }
            let now = CACurrentMediaTime()
            // Keep direct render-mode requests on the same throttle as the gate.
            let allowed = lastKeyframeRequestAt.map { now >= $0 + 0.3 } ?? true
            if allowed {
                lastKeyframeRequestAt = now
                keyframeRequests += 1
            }
            lock.unlock()
            if allowed { requester.request(reason: reason) }
        }
    }

    private func applyRenderMode(_ next: PipRenderMode, generation expected: UInt64) {
        guard isActive(expected) else { return }
        let steps = renderSwitch.request(next)
        guard !steps.isEmpty else { return }
        RsLog.marker("SESSVIEW", ["passthrough": next == .passthrough])
        for step in steps {
            guard isActive(expected) else { return }
            switch step {
            case .flushKeepingImage:
                renderer.flush()
            case .invalidateDecoder:
                decoder.invalidate()
            case .requestKeyframe:
                requestRenderSwitchKeyframe(generation: expected)
            }
        }
    }

    private func requestRenderSwitchKeyframe(generation expected: UInt64) {
        request([.decodeError], generation: expected)
    }

    private func scheduleDrainLocked(generation expected: UInt64) {
        guard !drainScheduled, !geometryPresentationPending, gate.count > 0 else { return }
        drainScheduled = true
        queue.async { [self] in drainOne(generation: expected) }
    }

    private func drainOne(generation expected: UInt64) {
        lock.lock()
        guard accepting, generation == expected else { lock.unlock(); return }
        guard !geometryPresentationPending else {
            drainScheduled = false
            lock.unlock()
            return
        }
        // A drain scheduled by the previous epoch can precede handle(config)
        // even though ingress has already replaced its pending frames. Leave
        // those frames queued until their source geometry has been installed.
        if let configEpoch, epochGeometry?.epoch != configEpoch {
            drainScheduled = false
            lock.unlock()
            return
        }
        let frame: SessionFrame?
        if let meta = gate.next(now: CACurrentMediaTime()),
           let index = pending.firstIndex(where: { $0.frameId == meta.frameId && $0.epoch == meta.epoch }) {
            frame = pending.remove(at: index)
        } else {
            frame = nil
        }
        lock.unlock()
        if let frame, isCurrent(epoch: frame.epoch, generation: expected) {
            switch renderSwitch.route(keyframe: frame.keyframe) {
            case .drop:
                lock.lock()
                if accepting, generation == expected, pendingEpoch == frame.epoch {
                    dropped += 1
                }
                lock.unlock()
            case .decode:
                decode(frame, generation: expected)
            case .passthrough:
                passthrough(frame, generation: expected)
            }
        }
        lock.lock()
        if accepting, generation == expected {
            drainScheduled = false
            scheduleDrainLocked(generation: expected)
        }
        lock.unlock()
    }

    private func prepareDecoder(epoch: UInt32) {
        guard decoderEpoch != epoch else { return }
        if decoderEpoch != nil {
            lock.lock()
            let presenting = geometryPresentationPending
            let previousSource = displayedGeometry?.src ?? CGRect(x: 0, y: 0, width: 1, height: 1)
            lock.unlock()
            // Preserve the old picture and its queue until a changed crop's
            // first decoded sample can be handed to the main-thread layout.
            let sourceChanged = epochGeometry?.epoch == epoch && epochGeometry?.src != previousSource
            if !presenting && !sourceChanged { renderer.flush() }
            RsLog.marker("SESSVIEW", ["epoch": Int(epoch), "rebuild": true])
        }
        decoder.invalidate()
        decoderEpoch = epoch
        firstFramePending = true
    }

    private func passthrough(_ frame: SessionFrame, generation expected: UInt64) {
        guard isCurrent(epoch: frame.epoch, generation: expected) else { return }
        prepareDecoder(epoch: frame.epoch)
        guard isCurrent(epoch: frame.epoch, generation: expected) else { return }
        let sample = decoder.makeCompressedSample(frame)
        guard isCurrent(epoch: frame.epoch, generation: expected) else { return }
        lastDecodeMs = 0
        guard let sample else {
            onDecode?(frame, 0, false)
            recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected, droppedCurrent: true)
            return
        }
        onDecode?(frame, 0, true)

        var size = CGSize.zero
        if let description = CMSampleBufferGetFormatDescription(sample) {
            let dimensions = CMVideoFormatDescriptionGetDimensions(description)
            size = CGSize(width: Int(dimensions.width), height: Int(dimensions.height))
        }
        let geometry = geometryForFrame(frame, decoded: size)
        if needsGeometryPresentation(geometry) {
            presentGeometry(geometry, sample: sample, image: nil, frame: frame, epoch: frame.epoch,
                            decodeUs: 0, decoderReady: true, generation: expected)
            return
        }

        lock.lock()
        guard accepting, generation == expected, pendingEpoch == frame.epoch else { lock.unlock(); return }
        gate.decoderReady(true)
        let enqueued = renderer.enqueueCompressed(sample)
        if enqueued { displayedGeometry = geometry }
        let attached = attachedAtUs
        let queueDepth = gate.count
        lock.unlock()
        guard enqueued else {
            renderer.flush()
            decoder.invalidate()
            recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected, droppedCurrent: true)
            return
        }

        stats.recordFrame(decodeMs: 0, latencyMs: latencyMs(frame), presentedAt: nil)
        didPresent(epoch: frame.epoch, frame: frame, size: size, decodeUs: 0, attachedAtUs: attached,
                   queueDepth: queueDepth, generation: expected)
    }

    private func decode(_ frame: SessionFrame, generation expected: UInt64) {
        guard isCurrent(epoch: frame.epoch, generation: expected) else { return }
        prepareDecoder(epoch: frame.epoch)
        let decoded = decoder.decode(frame)
        guard isCurrent(epoch: frame.epoch, generation: expected) else { return }
        if let duration = decoder.lastDecodeMs {
            lastDecodeMs = duration
            stats.recordFrame(decodeMs: duration, latencyMs: latencyMs(frame), presentedAt: nil)
        }
        onDecode?(frame, decoder.lastDecodeMs ?? 0, decoded != nil)
        guard let decoded else {
            recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected,
                    unavailable: decoder.lastFailure == .notReady, droppedCurrent: true)
            return
        }
        presentDecoded(decoded, epoch: frame.epoch, frame: frame, generation: expected)
    }

    /// Shared uncompressed presentation tail for VideoToolbox output and raw
    /// adapter buffers. Geometry changes are handed to the same main-thread
    /// transaction used by the compressed-session path.
    private func presentDecoded(_ decoded: SessionDecodedFrame, epoch: UInt32, frame: SessionFrame?,
                                generation expected: UInt64) {
        guard isCurrent(epoch: epoch, generation: expected) else { return }
        lock.lock()
        let presenting = geometryPresentationPending
        lock.unlock()
        guard !presenting else { return }
        if frame == nil {
            lastDecodeMs = decoded.decodeMs
            stats.recordFrame(decodeMs: decoded.decodeMs, latencyMs: nil, presentedAt: nil)
        }
        let size = CGSize(width: CVPixelBufferGetWidth(decoded.image), height: CVPixelBufferGetHeight(decoded.image))
        let geometry = geometryForEpoch(epoch, decoded: size)
        let decodeUs = Self.microseconds(decoded.decodeMs)
        if needsGeometryPresentation(geometry) {
            guard let sample = Self.makeDisplaySample(decoded) else {
                if let frame {
                    recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected, droppedCurrent: true)
                } else {
                    renderer.flush()
                }
                return
            }
            presentGeometry(geometry, sample: sample, image: decoded.image, frame: frame, epoch: epoch,
                            decodeUs: decodeUs, decoderReady: decoder.hasSession, generation: expected)
            return
        }

        // Stop/config invalidation cannot cross renderer enqueue. This never
        // holds the ingress lock around VideoToolbox decoding or UI dispatch.
        lock.lock()
        guard accepting, generation == expected, pendingEpoch == epoch else { lock.unlock(); return }
        if frame != nil { gate.decoderReady(decoder.hasSession) }
        let enqueued = renderer.enqueue(decoded)
        if enqueued {
            latestFrameBuffer = decoded.image
            displayedGeometry = geometry
        }
        let attached = attachedAtUs
        let queueDepth = gate.count
        lock.unlock()
        guard enqueued else {
            renderer.flush()
            if let frame {
                decoder.invalidate()
                recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected, droppedCurrent: true)
            }
            return
        }
        didPresent(epoch: epoch, frame: frame, size: size, decodeUs: decodeUs, attachedAtUs: attached,
                   queueDepth: queueDepth, generation: expected)
    }

    private func geometryForFrame(_ frame: SessionFrame, decoded: CGSize) -> EpochGeometry {
        geometryForEpoch(frame.epoch, decoded: decoded)
    }

    private func geometryForEpoch(_ epoch: UInt32, decoded: CGSize) -> EpochGeometry {
        let src = epochGeometry?.epoch == epoch ? epochGeometry?.src : nil
        return EpochGeometry(epoch: epoch, src: src ?? CGRect(x: 0, y: 0, width: 1, height: 1),
                             decoded: decoded)
    }

    private func needsGeometryPresentation(_ geometry: EpochGeometry) -> Bool {
        lock.lock()
        let src = displayedGeometry?.src ?? CGRect(x: 0, y: 0, width: 1, height: 1)
        lock.unlock()
        return src != geometry.src
    }

    /// At most one sample is handed to the UI. Keep latestFrameBuffer on the
    /// old displayed frame until the callback has captured its full-screen
    /// backdrop; no later frame can overtake this sample on the decode queue.
    private func presentGeometry(_ geometry: EpochGeometry, sample: CMSampleBuffer, image: CVPixelBuffer?,
                                 frame: SessionFrame?, epoch: UInt32, decodeUs: UInt64, decoderReady: Bool,
                                 generation expected: UInt64) {
        lock.lock()
        guard accepting, generation == expected, pendingEpoch == epoch else { lock.unlock(); return }
        geometryPresentationPending = true
        if frame != nil { gate.decoderReady(decoderReady) }
        let attached = attachedAtUs
        let queueDepth = gate.count
        lock.unlock()
        Self.setDisplayImmediately(sample)
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            var enqueued = false
            if self.isCurrent(epoch: epoch, generation: expected) {
                if let onEpochGeometry = self.onEpochGeometry {
                    onEpochGeometry(geometry, sample)
                    enqueued = !self.renderer.hasFailed
                } else {
                    enqueued = self.renderer.enqueueCompressed(sample)
                }
                self.lock.lock()
                if enqueued, self.accepting, self.generation == expected {
                    self.displayedGeometry = geometry
                    self.latestFrameBuffer = image
                }
                self.lock.unlock()
            }
            let submitted = enqueued
            self.queue.async { [self] in
                self.lock.lock()
                guard self.generation == expected else { self.lock.unlock(); return }
                self.geometryPresentationPending = false
                self.lock.unlock()
                if self.isCurrent(epoch: epoch, generation: expected) {
                    if submitted {
                        if image == nil, let frame {
                            self.stats.recordFrame(decodeMs: 0, latencyMs: self.latencyMs(frame), presentedAt: nil)
                        }
                        self.didPresent(epoch: epoch, frame: frame, size: geometry.decoded, decodeUs: decodeUs, attachedAtUs: attached,
                                        queueDepth: queueDepth, geometryChanged: true, generation: expected)
                    } else {
                        if let frame {
                            self.decoder.invalidate()
                            self.recover(frameId: frame.frameId, epoch: frame.epoch, generation: expected, droppedCurrent: true)
                        } else {
                            self.renderer.flush()
                        }
                    }
                }
                self.lock.lock()
                if self.accepting, self.generation == expected { self.scheduleDrainLocked(generation: expected) }
                self.lock.unlock()
            }
        }
    }

    /// Matches SessionFrameRenderer's uncompressed sample construction, but
    /// leaves enqueue to the surface's geometry transaction.
    private static func makeDisplaySample(_ decoded: SessionDecodedFrame) -> CMSampleBuffer? {
        var description: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: decoded.image, formatDescriptionOut: &description) == noErr,
              let description else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: decoded.presentationTime,
                                        decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: decoded.image, formatDescription: description,
            sampleTiming: &timing, sampleBufferOut: &sample) == noErr else { return nil }
        return sample
    }

    private static func setDisplayImmediately(_ sample: CMSampleBuffer) {
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            // SAFETY: The sample retains its array of mutable attachment dictionaries.
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dictionary,
                                 Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
    }

    /// Called only on the decode queue, after the sample was enqueued.
    private func didPresent(epoch: UInt32, frame: SessionFrame?, size: CGSize, decodeUs: UInt64,
                            attachedAtUs: UInt64, queueDepth: Int, geometryChanged: Bool = false,
                            generation expected: UInt64) {
        guard isCurrent(epoch: epoch, generation: expected) else { return }
        if let frame {
            let submittedAtUs = rs_now_us()
            let latencyUs = submittedAtUs >= frame.receivedUs ? submittedAtUs - frame.receivedUs : 0
            reporter.report(decodeUs: decodeUs, latencyUs: latencyUs, queue: UInt32(clamping: queueDepth))
            lastFrame = (frame.frameId, frame.epoch)
            codec = codecName(frame.codec)
        }
        let presentedAt = CACurrentMediaTime()
        stats.recordPresented(at: presentedAt)
        let width = Int(size.width)
        let height = Int(size.height)
        if geometryChanged {
            videoSize = size // The main-thread callback already applied it.
        } else if width > 0, height > 0 {
            reportVideoSize(size, generation: expected)
        }
        if firstFramePending {
            firstFramePending = false
            let displayed = rs_now_us()
            let ms = Double(displayed >= attachedAtUs ? displayed - attachedAtUs : 0) / 1_000
            RsLog.marker("SESSVIEW-FIRSTFRAME", ["epoch": Int(epoch), "width": width, "height": height, "ms": ms])
            DispatchQueue.main.async { [weak self] in
                guard let self, self.isCurrent(epoch: epoch, generation: expected) else { return }
                self.onFirstFrame?(epoch, width, height)
            }
        }
    }

    private func latencyMs(_ frame: SessionFrame) -> Double? {
        guard frame.clockSynced else { return nil }
        // The ABI's synchronized timestamp is signed. Use floating-point
        // subtraction to avoid overflow when a clock sample is out of range.
        return max(0, (Double(rs_now_us()) - Double(frame.captureLocalUs)) / 1_000)
    }

    private func recover(frameId: UInt32, epoch: UInt32, generation expected: UInt64,
                         unavailable: Bool = false, droppedCurrent: Bool = false) {
        lock.lock()
        guard accepting, generation == expected, pendingEpoch == epoch else { lock.unlock(); return }
        if droppedCurrent { dropped += 1 }
        gate.decoderReady(decoder.hasSession)
        let now = CACurrentMediaTime()
        let actions = unavailable ? gate.decoderUnavailable(now: now) : gate.decodeFailed(frameId: frameId, now: now)
        let reasons = applyLocked(actions)
        lock.unlock()
        request(reasons, generation: expected)
    }

    private func reportVideoSize(_ size: CGSize, generation expected: UInt64) {
        guard videoSize != size else { return }
        videoSize = size
        DispatchQueue.main.async { [weak self] in
            // These callbacks are queued before any later geometry handoff.
            // Do not drop an earlier notification when a superseding epoch
            // has the same size: videoSize is cached on the decode queue.
            guard let self, self.isActive(expected) else { return }
            self.onVideoSize?(size)
        }
    }

    private func handle(_ event: SessionEvent, generation expected: UInt64) {
        if event.kind == RS_EV_CLOSED {
            lock.lock()
            let current = generation == expected
            lock.unlock()
            guard current else { return }
            cancelTimer()
            decoder.invalidate()
            RsLog.marker("SESSVIEW", ["closed": true])
            DispatchQueue.main.async { [weak self] in self?.onClosed?() }
            return
        }
        guard isActive(expected) else { return }
        switch event.kind {
        case RS_EV_STREAM_CONFIG:
            guard isCurrent(epoch: event.epoch, generation: expected) else { return }
            let geometry = EpochGeometry(event)
            epochGeometry = geometry
            prepareDecoder(epoch: event.epoch)
            codec = codecName(event.codec)
            lock.lock()
            let presenting = geometryPresentationPending
            lock.unlock()
            if !presenting && !needsGeometryPresentation(geometry) {
                reportVideoSize(geometry.decoded, generation: expected)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.isActive(expected) else { return }
                self.onCursor?(event)
            }
            lock.lock()
            if accepting, generation == expected { scheduleDrainLocked(generation: expected) }
            lock.unlock()
        case RS_EV_CURSOR_POS, RS_EV_CURSOR_SHAPE, RS_EV_CURSOR_USE, RS_EV_CURSOR_HIDDEN:
            DispatchQueue.main.async { [weak self] in self?.onCursor?(event) }
        default:
            logger.debug("Session event kind=\(event.kind)")
        }
    }

    /// Start only after the first callback proves attach succeeded. Failed
    /// attach attempts have no callbacks and therefore no running timer.
    private func startTimerLocked(generation expected: UInt64) {
        guard !timerScheduled else { return }
        timerScheduled = true
        queue.async { [self] in
            guard isActive(expected) else { return }
            cancelTimer()
            let now = CACurrentMediaTime()
            nextStatsAt = now + 1
            nextLogAt = now + 5
            let source = DispatchSource.makeTimerSource(queue: queue)
            source.schedule(deadline: .now() + .milliseconds(100), repeating: .milliseconds(100))
            source.setEventHandler { [weak self] in
                guard let self, self.isActive(expected) else { return }
                self.tick(generation: expected)
            }
            timer = source
            source.resume()
        }
    }

    private func tick(generation expected: UInt64) {
        let now = CACurrentMediaTime()
        lock.lock()
        let presenting = geometryPresentationPending
        let raw = rawMode
        lock.unlock()
        if !presenting, renderer.hasFailed {
            if raw {
                renderer.flush()
            } else if let lastFrame {
                renderer.flush()
                decoder.invalidate()
                recover(frameId: lastFrame.id, epoch: lastFrame.epoch, generation: expected)
            }
        }
        if !raw {
            lock.lock()
            let reasons = accepting && generation == expected ? applyLocked(gate.tick(now: now)) : []
            lock.unlock()
            request(reasons, generation: expected)
        }
        if now >= nextStatsAt {
            repeat { nextStatsAt += 1 } while nextStatsAt <= now
            let payload = snapshot(at: now).dictionary()
            DispatchQueue.main.async { [weak self] in
                guard let self, self.isActive(expected) else { return }
                self.onVideoStats?(payload)
            }
        }
        if now >= nextLogAt {
            repeat { nextLogAt += 5 } while nextLogAt <= now
            RsLog.marker("SESSVIEW-STATS", snapshot(at: now).fields)
        }
    }

    private func snapshot(at now: TimeInterval) -> SessionVideoStats {
        let measurements = stats.snapshot(at: now)
        lock.lock()
        let raw = rawMode
        let result = SessionVideoStats(fps: measurements.fps, decodeP50Ms: measurements.decodeP50Ms,
                                      decodeP95Ms: measurements.decodeP95Ms, latencyP50Ms: measurements.latencyP50Ms,
                                      queue: raw ? 0 : gate.count, dropped: dropped,
                                      keyframeRequests: raw ? 0 : keyframeRequests,
                                      width: Int(videoSize.width), height: Int(videoSize.height), codec: codec)
        lock.unlock()
        return result
    }

    private func cancelTimer() { timer?.cancel(); timer = nil }

    private func codecName(_ value: UInt8) -> String {
        switch value {
        case 0: return "raw"
        case 2: return "h264"
        default: return "hevc"
        }
    }

    private static func microseconds(_ milliseconds: Double) -> UInt64 {
        guard milliseconds.isFinite, milliseconds > 0 else { return 0 }
        let value = milliseconds * 1_000
        guard value < Double(UInt64.max) else { return UInt64.max }
        return UInt64(value.rounded())
    }

    private func isActive(_ expected: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return accepting && generation == expected
    }

    private func isCurrent(epoch: UInt32, generation expected: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return accepting && generation == expected && pendingEpoch == epoch
    }

    deinit { timer?.cancel() }
}
