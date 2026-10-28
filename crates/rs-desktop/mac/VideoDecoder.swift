import AVFoundation
@preconcurrency import AppKit
import CoreMedia
import Foundation
import QuartzCore
import VideoToolbox

/// Per-frame completion callback shared with the Rust `FrameSink`.
public typealias RsDtFrameDone = @convention(c) (UnsafeMutableRawPointer?, Bool, UInt64) -> Void

@frozen public struct RsDtVideoDebug {
    public var decoded: UInt64
    public var failed: UInt64
    public var lastWidth: Int32
    public var lastHeight: Int32
    public var cursorX: Double
    public var cursorY: Double

    public init(decoded: UInt64 = 0, failed: UInt64 = 0, lastWidth: Int32 = 0,
                lastHeight: Int32 = 0, cursorX: Double = 0, cursorY: Double = 0) {
        self.decoded = decoded
        self.failed = failed
        self.lastWidth = lastWidth
        self.lastHeight = lastHeight
        self.cursorX = cursorX
        self.cursorY = cursorY
    }
}

private struct MacDecodedFrame {
    let image: CVPixelBuffer
    let presentationTime: CMTime
    let decodeUs: UInt64
}

/// Confined to `RsDtVideoPipeline`'s serial decode queue.
private final class MacVideoDecoder {
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private var codec: Int32 = -1

    func decode(data: Data, codec: Int32, keyframe: Bool, vps: Data, sps: Data, pps: Data,
                captureTsUs: UInt64) -> MacDecodedFrame? {
        guard (codec == 0 || codec == 1), validAvcc(data, codec: codec) else { return nil }
        let hasParameterSets = !vps.isEmpty || !sps.isEmpty || !pps.isEmpty
        if keyframe {
            let sets: [Data]
            if codec == 0 {
                guard !vps.isEmpty, !sps.isEmpty, !pps.isEmpty else { return nil }
                sets = [vps, sps, pps]
            } else {
                guard !sps.isEmpty, !pps.isEmpty else { return nil }
                sets = [sps, pps]
            }
            if session == nil || self.codec != codec || parameterSets != sets {
                guard makeSession(sets, codec: codec) else {
                    invalidate()
                    return nil
                }
            }
        } else if hasParameterSets || session == nil || self.codec != codec {
            return nil
        }
        guard let session, let format, !data.isEmpty,
              let sample = makeSampleBuffer(data, format: format, timestampUs: captureTsUs) else {
            return nil
        }

        let started = CACurrentMediaTime()
        var output: (image: CVPixelBuffer, pts: CMTime)?
        var outputStatus: OSStatus = noErr
        // Without the asynchronous-decode flag, VideoToolbox invokes the
        // output handler before this call returns, as in the iOS decoder.
        let status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [],
                                                        infoFlagsOut: nil) {
            status, _, image, pts, _ in
            outputStatus = status
            guard status == noErr, let image else { return }
            output = (image, pts)
        }
        let elapsed = max(0, (CACurrentMediaTime() - started) * 1_000_000)
        guard status == noErr, outputStatus == noErr, let output else {
            if status == kVTInvalidSessionErr || outputStatus == kVTInvalidSessionErr {
                invalidate()
            }
            return nil
        }
        return MacDecodedFrame(image: output.image, presentationTime: output.pts,
                               decodeUs: max(1, UInt64(elapsed)))
    }

    func invalidate() {
        if let session { VTDecompressionSessionInvalidate(session) }
        session = nil
        format = nil
        parameterSets.removeAll(keepingCapacity: true)
        codec = -1
    }

    private func validAvcc(_ data: Data, codec: Int32) -> Bool {
        guard !data.isEmpty else { return false }
        return data.withUnsafeBytes { bytes in
            let nal = bytes.bindMemory(to: UInt8.self)
            var offset = 0
            while offset < nal.count {
                guard nal.count - offset >= 4 else { return false }
                let count = (Int(nal[offset]) << 24) | (Int(nal[offset + 1]) << 16)
                    | (Int(nal[offset + 2]) << 8) | Int(nal[offset + 3])
                offset += 4
                guard count > 0, count <= nal.count - offset, nal[offset] & 0x80 == 0 else {
                    return false
                }
                if codec == 0 && (count < 2 || nal[offset + 1] & 0x07 == 0) { return false }
                offset += count
            }
            return true
        }
    }

    private func makeSession(_ sets: [Data], codec: Int32) -> Bool {
        guard let description = makeFormatDescription(sets, codec: codec) else { return false }
        invalidate()
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferIOSurfacePropertiesKey: [String: Any](),
        ]
        var newSession: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: description,
            decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary,
            outputCallback: nil,
            decompressionSessionOut: &newSession)
        guard status == noErr, let newSession else { return false }
        VTSessionSetProperty(newSession, key: kVTDecompressionPropertyKey_RealTime,
                             value: kCFBooleanTrue)
        session = newSession
        format = description
        parameterSets = sets
        self.codec = codec
        return true
    }

    private func makeFormatDescription(_ sets: [Data], codec: Int32) -> CMVideoFormatDescription? {
        guard (codec == 0 && sets.count == 3) || (codec == 1 && sets.count == 2) else {
            return nil
        }
        var description: CMVideoFormatDescription?
        let status: OSStatus
        if codec == 0 {
            status = sets[0].withUnsafeBytes { vps in
                sets[1].withUnsafeBytes { sps in
                    sets[2].withUnsafeBytes { pps in
                        guard let v = vps.baseAddress, let s = sps.baseAddress,
                              let p = pps.baseAddress else { return OSStatus(-1) }
                        let pointers = [v, s, p].map { $0.assumingMemoryBound(to: UInt8.self) }
                        return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                            allocator: kCFAllocatorDefault,
                            parameterSetCount: 3,
                            parameterSetPointers: pointers,
                            parameterSetSizes: sets.map(\.count),
                            nalUnitHeaderLength: 4,
                            extensions: nil,
                            formatDescriptionOut: &description)
                    }
                }
            }
        } else {
            status = sets[0].withUnsafeBytes { sps in
                sets[1].withUnsafeBytes { pps in
                    guard let s = sps.baseAddress, let p = pps.baseAddress else {
                        return OSStatus(-1)
                    }
                    let pointers = [s, p].map { $0.assumingMemoryBound(to: UInt8.self) }
                    return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                        allocator: kCFAllocatorDefault,
                        parameterSetCount: 2,
                        parameterSetPointers: pointers,
                        parameterSetSizes: sets.map(\.count),
                        nalUnitHeaderLength: 4,
                        formatDescriptionOut: &description)
                }
            }
        }
        guard status == noErr else { return nil }
        return description
    }

    private func makeSampleBuffer(_ avcc: Data, format: CMVideoFormatDescription,
                                  timestampUs: UInt64) -> CMSampleBuffer? {
        var block: CMBlockBuffer?
        var status = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: nil,
            blockLength: avcc.count,
            blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: avcc.count,
            flags: kCMBlockBufferAssureMemoryNowFlag,
            blockBufferOut: &block)
        guard status == noErr, let block else { return nil }
        status = avcc.withUnsafeBytes { bytes in
            guard let address = bytes.baseAddress else { return OSStatus(-1) }
            return CMBlockBufferReplaceDataBytes(with: address, blockBuffer: block,
                                                 offsetIntoDestination: 0,
                                                 dataLength: avcc.count)
        }
        guard status == noErr else { return nil }

        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMTime(value: CMTimeValue(clamping: timestampUs),
                                          timescale: 1_000_000),
            decodeTimeStamp: .invalid)
        var size = avcc.count
        var sample: CMSampleBuffer?
        status = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: block,
            formatDescription: format,
            sampleCount: 1,
            sampleTimingEntryCount: 1,
            sampleTimingArray: &timing,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &size,
            sampleBufferOut: &sample)
        guard status == noErr else { return nil }
        return sample
    }

    deinit { invalidate() }
}

/// A renderer that keeps the macOS 13 and macOS 14 display-layer APIs in one
/// place.  The caller confines it to the decode queue.
private final class MacVideoRenderer {
    private let layer: AVSampleBufferDisplayLayer

    init(layer: AVSampleBufferDisplayLayer) { self.layer = layer }

    func enqueue(_ decoded: MacDecodedFrame) -> Bool {
        var description: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: decoded.image,
            formatDescriptionOut: &description) == noErr,
            let description else { return false }
        var timing = CMSampleTimingInfo(duration: .invalid,
                                        presentationTimeStamp: decoded.presentationTime,
                                        decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: decoded.image,
            formatDescription: description,
            sampleTiming: &timing,
            sampleBufferOut: &sample) == noErr,
            let sample else { return false }
        return enqueueSample(sample)
    }

    func flush() {
        if #available(macOS 14.0, *) {
            layer.sampleBufferRenderer.flush()
        } else {
            layer.flush()
        }
    }

    private func enqueueSample(_ sample: CMSampleBuffer) -> Bool {
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            // SAFETY: CoreMedia's attachments array contains CFDictionary
            // objects; createIfNecessary makes the sample dictionary mutable.
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0),
                                           to: CFMutableDictionary.self)
            CFDictionarySetValue(
                dictionary,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }

        if #available(macOS 14.0, *) {
            let renderer = layer.sampleBufferRenderer
            if renderer.status == .failed {
                renderer.flush()
                return false
            }
            if renderer.requiresFlushToResumeDecoding { renderer.flush() }
            renderer.enqueue(sample)
            if renderer.status == .failed {
                renderer.flush()
                return false
            }
            return true
        }

        if layer.status == .failed {
            layer.flush()
            return false
        }
        if layer.requiresFlushToResumeDecoding { layer.flush() }
        layer.enqueue(sample)
        if layer.status == .failed {
            layer.flush()
            return false
        }
        return true
    }
}

/// VideoToolbox decode and AppKit display pipeline used by the Mac session.
public final class RsDtVideoPipeline {
    private let view: RsDtVideoView
    private let decoder: MacVideoDecoder
    private let renderer: MacVideoRenderer
    private let queue = DispatchQueue(label: "ai.gxlself.remote.desktop-video",
                                      qos: .userInteractive, autoreleaseFrequency: .workItem)
    private let lock = NSLock()
    private let headless: Bool
    private var closed = false
    private var decoded: UInt64 = 0
    private var failed: UInt64 = 0
    private var lastWidth: Int32 = 0
    private var lastHeight: Int32 = 0
    private var currentCodec: Int32 = -1
    private var generation: UInt64 = 0

    public convenience init(view: RsDtVideoView) {
        self.init(view: view, headless: false)
    }

    fileprivate init(view: RsDtVideoView, headless: Bool) {
        self.view = view
        self.headless = headless
        decoder = MacVideoDecoder()
        renderer = MacVideoRenderer(layer: view.videoDisplayLayer)
    }

    fileprivate func configure(codec: Int32, width: UInt32, height: UInt32) {
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        currentCodec = codec
        generation &+= 1
        // Enqueue while holding the ingress lock so a concurrent submit
        // cannot overtake the decoder reset for this configuration.
        queue.async { [self] in
            decoder.invalidate()
            renderer.flush() // Keep the last displayed image.
        }

        if headless {
            view.setVideoSize(width: width, height: height)
        } else {
            onMainAsync { [weak self] in
                self?.view.setVideoSize(width: width, height: height)
            }
        }

        lock.unlock()
    }

    fileprivate var acceptsFrames: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !closed
    }

    fileprivate func submit(frameId _: UInt32, keyframe: Bool, data: Data, vps: Data,
                            sps: Data, pps: Data, captureTsUs: UInt64,
                            ctx: UnsafeMutableRawPointer?, done: RsDtFrameDone) {
        lock.lock()
        guard !closed else {
            failed &+= 1
            lock.unlock()
            done(ctx, false, 0)
            return
        }
        let codec = currentCodec
        let expected = generation
        queue.async { [self] in
            autoreleasepool {
                process(data: data, codec: codec, keyframe: keyframe, vps: vps, sps: sps,
                        pps: pps, captureTsUs: captureTsUs, generation: expected,
                        ctx: ctx, done: done)
            }
        }
        lock.unlock()
    }

    private func process(data: Data, codec: Int32, keyframe: Bool, vps: Data, sps: Data,
                         pps: Data, captureTsUs: UInt64, generation expected: UInt64,
                         ctx: UnsafeMutableRawPointer?, done: RsDtFrameDone) {
        var ok = false
        var decodeUs: UInt64 = 0
        // One exit path owns the receipt, including closure and stale config.
        defer {
            lock.lock()
            if ok { decoded &+= 1 } else { failed &+= 1 }
            lock.unlock()
            done(ctx, ok, decodeUs)
        }
        guard isActive(expected),
              let frame = decoder.decode(data: data, codec: codec, keyframe: keyframe,
                                         vps: vps, sps: sps, pps: pps,
                                         captureTsUs: captureTsUs),
              isActive(expected), renderer.enqueue(frame) else { return }
        lock.lock()
        lastWidth = Int32(clamping: CVPixelBufferGetWidth(frame.image))
        lastHeight = Int32(clamping: CVPixelBufferGetHeight(frame.image))
        lock.unlock()
        ok = true
        decodeUs = frame.decodeUs
    }

    private func isActive(_ expected: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return !closed && generation == expected
    }

    public func close() {
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        closed = true
        queue.async { [self] in
            decoder.invalidate()
            renderer.flush()
        }
        lock.unlock()
    }

    fileprivate func debugState() -> RsDtVideoDebug {
        lock.lock()
        let values = (decoded, failed, lastWidth, lastHeight)
        lock.unlock()
        let point = view.remoteCursorPoint
        return RsDtVideoDebug(decoded: values.0, failed: values.1,
                              lastWidth: values.2, lastHeight: values.3,
                              cursorX: Double(point.x), cursorY: Double(point.y))
    }

    fileprivate func debugMoveCursor(nx: Double, ny: Double) {
        guard headless else { return }
        lock.lock()
        defer { lock.unlock() }
        view.showRemoteCursor(true)
        view.moveRemoteCursor(nx: nx, ny: ny)
    }

    deinit {
        // Queued closures keep the pipeline alive until they finish.  Never
        // enqueue a closure that retains `self` from deinit.
        decoder.invalidate()
        if !headless {
            onMainAsync { [view] in withExtendedLifetime(view) {} }
        }
    }
}

private func pipelineFromRaw(_ pointer: UnsafeMutableRawPointer?) -> RsDtVideoPipeline? {
    guard let pointer else { return nil }
    return Unmanaged<RsDtVideoPipeline>.fromOpaque(pointer).takeUnretainedValue()
}

private func copiedBytes(_ pointer: UnsafePointer<UInt8>?, _ length: Int) -> Data? {
    guard length >= 0 else { return nil }
    if length == 0 { return Data() }
    guard let pointer else { return nil }
    return Data(bytes: pointer, count: length)
}

@_cdecl("rs_dt_video_hw_supported")
public func rs_dt_video_hw_supported(_ codec: Int32) -> Bool {
    switch codec {
    case 0: return VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC)
    case 1: return VTIsHardwareDecodeSupported(kCMVideoCodecType_H264)
    default: return false
    }
}

@_cdecl("rs_dt_video_create_headless")
public func rs_dt_video_create_headless(_ width: Int32, _ height: Int32) -> UnsafeMutableRawPointer {
    autoreleasepool {
        let view = RsDtVideoView(frame: NSRect(x: 0, y: 0,
                                              width: CGFloat(max(0, width)),
                                              height: CGFloat(max(0, height))))
        let pipeline = RsDtVideoPipeline(view: view, headless: true)
        return Unmanaged.passRetained(pipeline).toOpaque()
    }
}

@_cdecl("rs_dt_video_configure")
public func rs_dt_video_configure(_ pointer: UnsafeMutableRawPointer?, _ codec: Int32,
                                  _ width: UInt32, _ height: UInt32) {
    autoreleasepool {
        pipelineFromRaw(pointer)?.configure(codec: codec, width: width, height: height)
    }
}

@_cdecl("rs_dt_video_submit")
public func rs_dt_video_submit(
    _ pointer: UnsafeMutableRawPointer?,
    _ frameId: UInt32,
    _ keyframe: Bool,
    _ data: UnsafePointer<UInt8>?,
    _ dataLen: Int,
    _ vps: UnsafePointer<UInt8>?,
    _ vpsLen: Int,
    _ sps: UnsafePointer<UInt8>?,
    _ spsLen: Int,
    _ pps: UnsafePointer<UInt8>?,
    _ ppsLen: Int,
    _ captureTsUs: UInt64,
    _ ctx: UnsafeMutableRawPointer?,
    _ done: RsDtFrameDone
) {
    guard let pipeline = pipelineFromRaw(pointer), pipeline.acceptsFrames else {
        done(ctx, false, 0)
        return
    }
    guard let data = copiedBytes(data, dataLen),
          let vps = copiedBytes(vps, vpsLen),
          let sps = copiedBytes(sps, spsLen),
          let pps = copiedBytes(pps, ppsLen) else {
        done(ctx, false, 0)
        return
    }
    // All borrowed Rust bytes have been copied on the calling thread before
    // the queue receives the frame or this FFI function returns.
    pipeline.submit(frameId: frameId, keyframe: keyframe, data: data, vps: vps, sps: sps, pps: pps,
                    captureTsUs: captureTsUs, ctx: ctx, done: done)
}

@_cdecl("rs_dt_video_close")
public func rs_dt_video_close(_ pointer: UnsafeMutableRawPointer?) {
    pipelineFromRaw(pointer)?.close()
}

@_cdecl("rs_dt_video_release")
public func rs_dt_video_release(_ pointer: UnsafeMutableRawPointer?) {
    guard let pointer else { return }
    let pipeline = Unmanaged<RsDtVideoPipeline>.fromOpaque(pointer).takeRetainedValue()
    pipeline.close()
}

@_cdecl("rs_dt_video_debug_state")
public func rs_dt_video_debug_state(_ pointer: UnsafeMutableRawPointer?,
                                    _ output: UnsafeMutableRawPointer?) {
    guard let output else { return }
    // As in Common.swift's storeBuf, keep a Swift struct out of @_cdecl's
    // signature while retaining the exact C layout of the output pointer.
    output.assumingMemoryBound(to: RsDtVideoDebug.self).pointee =
        pipelineFromRaw(pointer)?.debugState() ?? RsDtVideoDebug()
}

@_cdecl("rs_dt_video_debug_move_cursor")
public func rs_dt_video_debug_move_cursor(_ pointer: UnsafeMutableRawPointer?, _ nx: Double,
                                          _ ny: Double) {
    autoreleasepool {
        pipelineFromRaw(pointer)?.debugMoveCursor(nx: nx, ny: ny)
    }
}

// MARK: - Test-only VideoToolbox encoder

private final class RsDtTestEncoderContext {
    let codec: Int32
    private let lock = NSLock()
    private var frames: [(pts: Int64, data: Data)] = []

    init(codec: Int32) { self.codec = codec }

    func append(_ frame: Data, pts: CMTime) {
        lock.lock()
        frames.append((pts.value, frame))
        lock.unlock()
    }

    func snapshot() -> [Data] {
        lock.lock()
        defer { lock.unlock() }
        return frames.sorted { $0.pts < $1.pts }.map(\.data)
    }
}

private func rsDtTestFormat(_ description: CMVideoFormatDescription,
                            codec: Int32) -> (sets: [Data], nalLength: Int)? {
    let count: Int
    var nalLength: Int32 = 0
    var status: OSStatus
    if codec == 0 {
        var value = 0
        status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            description, parameterSetIndex: 0, parameterSetPointerOut: nil,
            parameterSetSizeOut: nil, parameterSetCountOut: &value,
            nalUnitHeaderLengthOut: &nalLength)
        count = value
    } else {
        var value = 0
        status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            description, parameterSetIndex: 0, parameterSetPointerOut: nil,
            parameterSetSizeOut: nil, parameterSetCountOut: &value,
            nalUnitHeaderLengthOut: &nalLength)
        count = value
    }
    guard status == noErr, count > 0, nalLength > 0, nalLength <= 4 else { return nil }

    var output: [Data] = []
    output.reserveCapacity(count)
    for index in 0..<count {
        var pointer: UnsafePointer<UInt8>?
        var size = 0
        if codec == 0 {
            status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                description, parameterSetIndex: index, parameterSetPointerOut: &pointer,
                parameterSetSizeOut: &size, parameterSetCountOut: nil,
                nalUnitHeaderLengthOut: nil)
        } else {
            status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                description, parameterSetIndex: index, parameterSetPointerOut: &pointer,
                parameterSetSizeOut: &size, parameterSetCountOut: nil,
                nalUnitHeaderLengthOut: nil)
        }
        guard status == noErr, let pointer, size > 0 else { return nil }
        output.append(Data(bytes: pointer, count: size))
    }
    return (output, Int(nalLength))
}

private func rsDtTestAnnexB(_ sample: CMSampleBuffer, codec: Int32, keyframe: Bool) -> Data? {
    guard let description = CMSampleBufferGetFormatDescription(sample),
          let format = rsDtTestFormat(description, codec: codec),
          let block = CMSampleBufferGetDataBuffer(sample) else { return nil }
    let total = CMBlockBufferGetDataLength(block)
    guard total > 0 else { return nil }
    var raw = Data(count: total)
    let copyStatus = raw.withUnsafeMutableBytes { bytes -> OSStatus in
        guard let base = bytes.baseAddress else { return OSStatus(-1) }
        return CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: total,
                                          destination: base)
    }
    guard copyStatus == noErr else { return nil }

    var output = Data()
    let startCode = Data([0, 0, 0, 1])
    if keyframe {
        for parameterSet in format.sets {
            output.append(startCode)
            output.append(parameterSet)
        }
    }
    var offset = 0
    raw.withUnsafeBytes { bytes in
        guard let base = bytes.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
        while total - offset >= format.nalLength {
            var size = 0
            for byte in 0..<format.nalLength { size = (size << 8) | Int(base[offset + byte]) }
            offset += format.nalLength
            guard size > 0, size <= total - offset else {
                offset = total + 1
                return
            }
            output.append(startCode)
            output.append(base + offset, count: size)
            offset += size
        }
    }
    return offset == total ? output : nil
}

private func rsDtTestEncodeCallback(
    _ outputCallbackRefCon: UnsafeMutableRawPointer?,
    _ sourceFrameRefCon: UnsafeMutableRawPointer?,
    _ status: OSStatus,
    _ infoFlags: VTEncodeInfoFlags,
    _ sampleBuffer: CMSampleBuffer?
) {
    _ = sourceFrameRefCon
    _ = infoFlags
    guard status == noErr, let sampleBuffer,
          let context = outputCallbackRefCon.map({
              Unmanaged<RsDtTestEncoderContext>.fromOpaque($0).takeUnretainedValue()
          }), CMSampleBufferDataIsReady(sampleBuffer) else { return }
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer,
                                                              createIfNecessary: false)
        as? [[String: Any]]
    let notSync = attachments?.first?[kCMSampleAttachmentKey_NotSync as String] as? Bool ?? false
    if let data = rsDtTestAnnexB(sampleBuffer, codec: context.codec, keyframe: !notSync) {
        context.append(data, pts: CMSampleBufferGetPresentationTimeStamp(sampleBuffer))
    }
}

private func rsDtTestPixelBuffer(index: UInt32) -> CVPixelBuffer? {
    var pixelBuffer: CVPixelBuffer?
    let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [String: Any]()]
    guard CVPixelBufferCreate(kCFAllocatorDefault, 320, 240,
                              kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                              attributes as CFDictionary, &pixelBuffer) == kCVReturnSuccess,
          let pixelBuffer,
          CVPixelBufferLockBaseAddress(pixelBuffer, []) == kCVReturnSuccess else { return nil }
    defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, []) }
    guard let yBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0)?.assumingMemoryBound(to: UInt8.self),
          let uvBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1)?.assumingMemoryBound(to: UInt8.self) else { return nil }
    let yStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
    let uvStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
    let luma = UInt8(32 + (index % 24) * 8)
    let cb = UInt8(112 + (index % 8) * 4)
    let cr = UInt8(144 - (index % 8) * 4)
    for y in 0..<240 {
        for x in 0..<320 { yBase[y * yStride + x] = luma }
    }
    for y in stride(from: 0, to: 240, by: 2) {
        for x in stride(from: 0, to: 320, by: 2) {
            let offset = (y / 2) * uvStride + x
            uvBase[offset] = cb
            uvBase[offset + 1] = cr
        }
    }
    return pixelBuffer
}

/// Encode `frameCount` synthetic 320x240 frames as length-prefixed Annex-B
/// access units for the macOS integration tests.  The returned status is zero
/// on success and the bytes are written through the `RsDtBuf` output pointer.
@_cdecl("rs_dt_test_encode")
public func rs_dt_test_encode(_ codec: Int32, _ frameCount: UInt32,
                              _ output: UnsafeMutableRawPointer?) -> Int32 {
    autoreleasepool { rsDtTestEncode(codec, frameCount, output) }
}

private func rsDtTestEncode(_ codec: Int32, _ frameCount: UInt32,
                            _ output: UnsafeMutableRawPointer?) -> Int32 {
    guard let output else { return -1 }
    output.assumingMemoryBound(to: RsDtBuf.self).pointee = RsDtBuf()
    guard (codec == 0 || codec == 1), frameCount > 0 else { return -1 }
    let cmCodec: CMVideoCodecType = codec == 0 ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264
    let context = RsDtTestEncoderContext(codec: codec)
    var session: VTCompressionSession?
    let specifications: [CFString: Any] = [
        kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true,
    ]
    var status = VTCompressionSessionCreate(
        allocator: kCFAllocatorDefault, width: 320, height: 240, codecType: cmCodec,
        encoderSpecification: specifications as CFDictionary, imageBufferAttributes: nil,
        compressedDataAllocator: nil, outputCallback: rsDtTestEncodeCallback,
        refcon: Unmanaged.passUnretained(context).toOpaque(), compressionSessionOut: &session)
    if status != noErr {
        status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault, width: 320, height: 240, codecType: cmCodec,
            encoderSpecification: nil, imageBufferAttributes: nil,
            compressedDataAllocator: nil, outputCallback: rsDtTestEncodeCallback,
            refcon: Unmanaged.passUnretained(context).toOpaque(), compressionSessionOut: &session)
    }
    guard status == noErr else { return status }
    guard let session else { return -2 }
    defer {
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
        // The C callback only borrows this object.  Keep it alive through
        // draining and invalidation, including every early return below.
        withExtendedLifetime(context) {}
    }
    let properties: [(CFString, CFTypeRef)] = [
        (kVTCompressionPropertyKey_RealTime, kCFBooleanTrue),
        (kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse),
        (kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: 5)),
        (kVTCompressionPropertyKey_ProfileLevel,
         codec == 0 ? kVTProfileLevel_HEVC_Main_AutoLevel : kVTProfileLevel_H264_High_AutoLevel),
    ]
    for (key, value) in properties {
        status = VTSessionSetProperty(session, key: key, value: value)
        guard status == noErr else { return status }
    }
    if codec == 0 {
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AllowOpenGOP,
                             value: kCFBooleanFalse)
    }
    status = VTCompressionSessionPrepareToEncodeFrames(session)
    guard status == noErr else { return status }

    for index in 0..<frameCount {
        guard let pixelBuffer = rsDtTestPixelBuffer(index: index) else { return -2 }
        let forceKeyframe = index % 5 == 0
        let options: CFDictionary? = forceKeyframe
            ? [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary
            : nil
        status = VTCompressionSessionEncodeFrame(
            session, imageBuffer: pixelBuffer,
            presentationTimeStamp: CMTime(value: CMTimeValue(index), timescale: 60),
            duration: .invalid, frameProperties: options, sourceFrameRefcon: nil,
            infoFlagsOut: nil)
        guard status == noErr else { return status }
    }
    status = VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
    guard status == noErr else { return status }
    let frames = context.snapshot()
    guard frames.count == Int(frameCount) else { return -3 }
    var outputData = Data()
    for frame in frames {
        var length = UInt32(frame.count).bigEndian
        withUnsafeBytes(of: &length) { outputData.append(contentsOf: $0) }
        outputData.append(frame)
    }
    storeBuf(output, outputData)
    return output.assumingMemoryBound(to: RsDtBuf.self).pointee.ptr == nil ? -4 : 0
}
