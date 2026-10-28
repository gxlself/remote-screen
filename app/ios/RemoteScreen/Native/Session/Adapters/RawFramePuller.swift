import CoreVideo
import Foundation
import QuartzCore

private final class RawFrameCallbackBox {
    weak var puller: RawFramePuller?
    let pipeline: SessionVideoPipeline

    init(puller: RawFramePuller) {
        self.puller = puller
        pipeline = puller.pipeline
    }
}

private let rawReadyCallback: RsRawReadyCb = { context, seq, width, height in
    guard let context else { return }
    let box = Unmanaged<RawFrameCallbackBox>.fromOpaque(context).takeUnretainedValue()
    box.puller?.ready(seq: seq, width: width, height: height)
}

private let rawEventCallback: RsEventCb = { context, event in
    guard let context, let event else { return }
    let box = Unmanaged<RawFrameCallbackBox>.fromOpaque(context).takeUnretainedValue()
    box.pipeline.submit(SessionEvent(copying: event.pointee))
}

/// Pulls adapter framebuffer updates on a private serial queue. Rust callbacks
/// only update pacing state and enqueue work; all pixel copying happens later.
final class RawFramePuller {
    private let lock = NSLock()
    private let pullQueue = DispatchQueue(label: "ai.gxlself.remote.session-raw", qos: .userInteractive)
    fileprivate let pipeline: SessionVideoPipeline

    private var attachedId: UInt64?
    private var context: UnsafeMutableRawPointer?
    private var generation: UInt64 = 0
    private var active = false
    private var width = 0
    private var height = 0
    private var latestReadySeq: UInt64 = 0
    private var pacing = AdapterRawPacing()
    private var ring = AdapterRawRing()
    private var buffers: [CVPixelBuffer?] = Array(repeating: nil, count: 3)

    init(pipeline: SessionVideoPipeline) {
        self.pipeline = pipeline
    }

    /// Must be called from the surface's main thread.
    func attach(id: UInt64) -> Bool {
        detach()

        let box = RawFrameCallbackBox(puller: self)
        let retained = Unmanaged.passRetained(box)
        let rawContext = retained.toOpaque()
        lock.lock()
        generation &+= 1
        let expected = generation
        active = true
        attachedId = id
        context = rawContext
        width = 0
        height = 0
        latestReadySeq = 0
        pacing.reset()
        ring.reset()
        buffers = Array(repeating: nil, count: 3)
        lock.unlock()

        guard rs_raw_attach(id, rawContext, rawReadyCallback, rawEventCallback) else {
            lock.lock()
            let ownsContext = generation == expected && context == rawContext
            if ownsContext {
                generation &+= 1
                active = false
                attachedId = nil
                context = nil
                pacing.reset()
                ring.reset()
                buffers = Array(repeating: nil, count: 3)
                latestReadySeq = 0
            }
            lock.unlock()
            if ownsContext { retained.release() }
            return false
        }
        return true
    }

    /// Must be called from the surface's main thread. The generation is
    /// invalidated before the Rust barrier so an in-flight pull is discarded.
    func detach() {
        lock.lock()
        generation &+= 1
        active = false
        let id = attachedId
        let rawContext = context
        attachedId = nil
        context = nil
        lock.unlock()

        if let id { rs_raw_detach(id) }
        if let rawContext {
            Unmanaged<RawFrameCallbackBox>.fromOpaque(rawContext).release()
        }

        lock.lock()
        pacing.reset()
        ring.reset()
        buffers = Array(repeating: nil, count: 3)
        width = 0
        height = 0
        latestReadySeq = 0
        lock.unlock()
    }

    fileprivate func ready(seq: UInt64, width: UInt32, height: UInt32) {
        lock.lock()
        guard active else {
            lock.unlock()
            return
        }
        if seq >= latestReadySeq {
            latestReadySeq = seq
            self.width = Int(width)
            self.height = Int(height)
        }
        let delay = pacing.ready(seq: seq, now: CACurrentMediaTime())
        let expected = generation
        lock.unlock()

        guard let delay else { return }
        schedulePull(after: delay, generation: expected)
    }

    private func schedulePull(after delay: TimeInterval, generation expected: UInt64) {
        pullQueue.asyncAfter(deadline: .now() + max(0, delay)) { [weak self] in
            self?.pull(generation: expected)
        }
    }

    private func pull(generation expected: UInt64) {
        lock.lock()
        guard active, generation == expected, let id = attachedId else {
            lock.unlock()
            return
        }
        _ = pacing.begin(now: CACurrentMediaTime())
        let slot = ring.next(width: width, height: height)
        let targetWidth = width
        let targetHeight = height
        let existing = slot.needsAlloc ? nil : buffers[slot.index]
        lock.unlock()

        var image = existing
        if image == nil {
            image = makeBuffer(width: targetWidth, height: targetHeight, generation: expected)
            guard image != nil else {
                finishPull(generation: expected, index: slot.index, seq: 0,
                           image: nil, copyMs: 0)
                return
            }
            lock.lock()
            guard active, generation == expected else {
                lock.unlock()
                return
            }
            buffers[slot.index] = image
            lock.unlock()
        }
        guard let image else {
            finishPull(generation: expected, index: slot.index, seq: 0,
                       image: nil, copyMs: 0)
            return
        }

        let lockStatus = CVPixelBufferLockBaseAddress(image, [])
        guard lockStatus == kCVReturnSuccess else {
            RsLog.marker("SESSVIEW", ["error": "raw-buffer-lock", "status": Int(lockStatus)])
            finishPull(generation: expected, index: slot.index, seq: 0,
                       image: nil, copyMs: 0)
            return
        }
        guard let base = CVPixelBufferGetBaseAddress(image) else {
            RsLog.marker("SESSVIEW", ["error": "raw-buffer-address"])
            CVPixelBufferUnlockBaseAddress(image, [])
            finishPull(generation: expected, index: slot.index, seq: 0,
                       image: nil, copyMs: 0)
            return
        }

        var rect = RsRect(x: 0, y: 0, w: 0, h: 0)
        let started = CACurrentMediaTime()
        let seq = rs_raw_copy(id, slot.sinceSeq, base.assumingMemoryBound(to: UInt8.self),
                              CVPixelBufferGetBytesPerRow(image), UInt32(targetWidth), UInt32(targetHeight),
                              UInt8(RS_PIX_BGRA8), &rect)
        let copyMs = max(0, (CACurrentMediaTime() - started) * 1_000)
        let hasPixels = rect.w > 0 && rect.h > 0
        CVPixelBufferUnlockBaseAddress(image, [])
        finishPull(generation: expected, index: slot.index, seq: seq,
                   image: hasPixels ? image : nil, copyMs: copyMs)
    }

    private func makeBuffer(width: Int, height: Int, generation expected: UInt64) -> CVPixelBuffer? {
        guard width > 0, height > 0 else { return nil }
        lock.lock()
        let stillActive = active && generation == expected
        lock.unlock()
        guard stillActive else { return nil }

        let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [String: Any]()]
        var image: CVPixelBuffer?
        let status = CVPixelBufferCreate(kCFAllocatorDefault, width, height,
                                         kCVPixelFormatType_32BGRA, attributes as CFDictionary, &image)
        guard status == kCVReturnSuccess, let image else {
            RsLog.marker("SESSVIEW", ["error": "raw-buffer-create", "status": Int(status)])
            return nil
        }
        return image
    }

    private func finishPull(generation expected: UInt64, index: Int, seq: UInt64,
                            image: CVPixelBuffer?, copyMs: Double) {
        lock.lock()
        guard active, generation == expected else {
            lock.unlock()
            return
        }
        ring.copied(index: index, seq: seq)
        // Submission only acquires the pipeline ingress lock and queues work.
        // Keeping our lock until it returns makes detach invalidate either
        // before this submission or after it, never between the check and call.
        if seq != 0, let image {
            pipeline.submitRaw(image: image, copyMs: copyMs)
        }
        let delay = pacing.end(pulledSeq: seq, now: CACurrentMediaTime())
        lock.unlock()
        if let delay { schedulePull(after: delay, generation: expected) }
    }

    deinit {
        detach()
    }
}
