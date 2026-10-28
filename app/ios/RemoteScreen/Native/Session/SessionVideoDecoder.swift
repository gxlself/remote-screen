import AVFoundation
import CoreMedia
import QuartzCore
import VideoToolbox

struct SessionDecodedFrame {
    let image: CVPixelBuffer
    let presentationTime: CMTime
    let decodeMs: Double
}

enum SessionVideoDecodeFailure {
    case notReady
    case decode
}

/// Confined to SessionVideoPipeline's serial decoding queue.
final class SessionVideoDecoder {
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private var codec: SessionCodecDecision = .noParameterSets
    private(set) var lastDecodeMs: Double?
    private(set) var lastFailure: SessionVideoDecodeFailure?

    var hasSession: Bool { session != nil }

    func decode(_ frame: SessionFrame) -> SessionDecodedFrame? {
        lastDecodeMs = nil
        lastFailure = .decode
        let decision = SessionCodec.detect(vpsLen: frame.vps.count, spsLen: frame.sps.count, ppsLen: frame.pps.count)
        if decision != .noParameterSets {
            let sets = decision == .hevc ? [frame.vps, frame.sps, frame.pps] : [frame.sps, frame.pps]
            if session == nil || sets != parameterSets || decision != codec {
                guard makeSession(sets, codec: decision) else {
                    invalidate()
                    lastFailure = .notReady
                    return nil
                }
            }
        }
        guard let session, let format else {
            lastFailure = .notReady
            return nil
        }
        guard !frame.avcc.isEmpty else { return nil }

        guard let sample = makeSampleBuffer(frame.avcc, format: format, timestampUs: frame.captureTimestampUs) else {
            return nil
        }

        let started = CACurrentMediaTime()
        var output: (image: CVPixelBuffer, pts: CMTime)?
        var outputStatus: OSStatus = noErr
        var status = noErr
        // No asynchronous decode flag: like M0, the output handler completes
        // before DecodeFrame returns. Pixels stay on the serial decode queue.
        status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [], infoFlagsOut: nil) {
            status, _, image, pts, _ in
            outputStatus = status
            guard status == noErr, let image else { return }
            output = (image, pts)
        }
        let duration = max(0, (CACurrentMediaTime() - started) * 1_000)
        lastDecodeMs = duration
        guard status == noErr, outputStatus == noErr, let output else {
            RsLog.marker("SESSVIEW", ["error": "decode", "status": Int(status != noErr ? status : outputStatus)])
            if status == kVTInvalidSessionErr || outputStatus == kVTInvalidSessionErr { invalidate() }
            return nil
        }
        lastFailure = nil
        return SessionDecodedFrame(image: output.image, presentationTime: output.pts, decodeMs: duration)
    }

    /// Builds a compressed sample for AVSampleBufferDisplayLayer. This path
    /// only maintains a format description; it never creates a VT session.
    func makeCompressedSample(_ frame: SessionFrame) -> CMSampleBuffer? {
        invalidateSessionOnly()
        lastDecodeMs = 0
        lastFailure = .decode
        let decision = SessionCodec.detect(vpsLen: frame.vps.count, spsLen: frame.sps.count, ppsLen: frame.pps.count)
        if decision != .noParameterSets {
            let sets = decision == .hevc ? [frame.vps, frame.sps, frame.pps] : [frame.sps, frame.pps]
            if format == nil || sets != parameterSets || decision != codec {
                guard let description = makeFormatDescription(sets, codec: decision) else {
                    lastFailure = .notReady
                    return nil
                }
                // A compressed sample does not use the VT session. Keep only
                // the description and its parameter-set identity for deltas.
                format = description
                parameterSets = sets
                codec = decision
            }
        }
        guard let format else {
            lastFailure = .notReady
            return nil
        }
        guard let sample = makeSampleBuffer(frame.avcc, format: format, timestampUs: frame.captureTimestampUs) else {
            return nil
        }
        lastFailure = nil
        return sample
    }

    func invalidate() {
        invalidateSessionOnly()
        format = nil
        parameterSets = []
        codec = .noParameterSets
    }

    private func invalidateSessionOnly() {
        if let session { VTDecompressionSessionInvalidate(session) }
        session = nil
    }

    private func makeSession(_ sets: [Data], codec: SessionCodecDecision) -> Bool {
        guard let description = makeFormatDescription(sets, codec: codec) else {
            return false
        }
        // Parameter changes always discard the previous decompression session.
        invalidate()
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferIOSurfacePropertiesKey: [String: Any](),
        ]
        var newSession: VTDecompressionSession?
        let created = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault, formatDescription: description, decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary, outputCallback: nil, decompressionSessionOut: &newSession)
        guard created == noErr, let newSession else {
            RsLog.marker("SESSVIEW", ["error": "decoder-create", "status": Int(created)])
            return false
        }
        VTSessionSetProperty(newSession, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        session = newSession
        format = description
        parameterSets = sets
        self.codec = codec
        return true
    }

    private func makeFormatDescription(_ sets: [Data], codec: SessionCodecDecision) -> CMVideoFormatDescription? {
        guard (codec == .hevc && sets.count == 3) || (codec == .h264 && sets.count == 2) else {
            RsLog.marker("SESSVIEW", ["error": "parameter-sets", "status": -1])
            return nil
        }
        var description: CMVideoFormatDescription?
        let status: OSStatus
        if codec == .hevc {
            status = sets[0].withUnsafeBytes { vps in
                sets[1].withUnsafeBytes { sps in
                    sets[2].withUnsafeBytes { pps in
                        guard let v = vps.baseAddress, let s = sps.baseAddress, let p = pps.baseAddress else {
                            return OSStatus(-1)
                        }
                        let pointers = [v, s, p].map { $0.assumingMemoryBound(to: UInt8.self) }
                        return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                            allocator: kCFAllocatorDefault, parameterSetCount: 3, parameterSetPointers: pointers,
                            parameterSetSizes: sets.map(\.count), nalUnitHeaderLength: 4, extensions: nil,
                            formatDescriptionOut: &description)
                    }
                }
            }
        } else {
            status = sets[0].withUnsafeBytes { sps in
                sets[1].withUnsafeBytes { pps in
                    guard let s = sps.baseAddress, let p = pps.baseAddress else { return OSStatus(-1) }
                    let pointers = [s, p].map { $0.assumingMemoryBound(to: UInt8.self) }
                    return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                        allocator: kCFAllocatorDefault, parameterSetCount: 2, parameterSetPointers: pointers,
                        parameterSetSizes: sets.map(\.count), nalUnitHeaderLength: 4,
                        formatDescriptionOut: &description)
                }
            }
        }
        guard status == noErr, let description else {
            RsLog.marker("SESSVIEW", ["error": "parameter-sets", "status": Int(status)])
            return nil
        }
        return description
    }

    private func makeSampleBuffer(_ avcc: Data, format: CMVideoFormatDescription,
                                  timestampUs: UInt64) -> CMSampleBuffer? {
        guard !avcc.isEmpty else { return nil }
        var block: CMBlockBuffer?
        var status = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: avcc.count,
            blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
            dataLength: avcc.count, flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &block)
        guard status == noErr, let block else { return nil }
        status = avcc.withUnsafeBytes { bytes in
            guard let address = bytes.baseAddress else { return OSStatus(-1) }
            return CMBlockBufferReplaceDataBytes(with: address, blockBuffer: block,
                                                offsetIntoDestination: 0, dataLength: avcc.count)
        }
        guard status == noErr else { return nil }

        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMTime(value: CMTimeValue(clamping: timestampUs), timescale: 1_000_000),
            decodeTimeStamp: .invalid)
        var size = avcc.count
        var sample: CMSampleBuffer?
        status = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format, sampleCount: 1,
            sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample)
        guard status == noErr, let sample else { return nil }
        return sample
    }

    deinit { invalidate() }
}

/// iOS 17 moved enqueueing onto `sampleBufferRenderer`; iOS 16 keeps it on the layer itself.
struct SampleBufferTarget {
    let layer: AVSampleBufferDisplayLayer

    var status: AVQueuedSampleBufferRenderingStatus {
        if #available(iOS 17, *) { return layer.sampleBufferRenderer.status }
        return layer.status
    }

    var requiresFlushToResumeDecoding: Bool {
        if #available(iOS 17, *) { return layer.sampleBufferRenderer.requiresFlushToResumeDecoding }
        return layer.requiresFlushToResumeDecoding
    }

    func enqueue(_ sample: CMSampleBuffer) {
        if #available(iOS 17, *) { layer.sampleBufferRenderer.enqueue(sample) } else { layer.enqueue(sample) }
    }

    func flush() {
        if #available(iOS 17, *) { layer.sampleBufferRenderer.flush() } else { layer.flush() }
    }

    func flushRemovingDisplayedImage() {
        if #available(iOS 17, *) {
            layer.sampleBufferRenderer.flush(removingDisplayedImage: true, completionHandler: nil)
        } else {
            layer.flushAndRemoveImage()
        }
    }
}

final class SessionFrameRenderer {
    private let renderer: SampleBufferTarget

    init(layer: AVSampleBufferDisplayLayer) { renderer = SampleBufferTarget(layer: layer) }

    var hasFailed: Bool { renderer.status == .failed }

    func enqueue(_ decoded: SessionDecodedFrame) -> Bool {
        var description: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: decoded.image, formatDescriptionOut: &description) == noErr,
              let description else { return false }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: decoded.presentationTime, decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: decoded.image, formatDescription: description,
            sampleTiming: &timing, sampleBufferOut: &sample) == noErr, let sample else { return false }
        return enqueueSample(sample)
    }

    func enqueueCompressed(_ sample: CMSampleBuffer) -> Bool {
        return enqueueSample(sample)
    }

    private func enqueueSample(_ sample: CMSampleBuffer) -> Bool {
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dictionary,
                                 Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        if renderer.status == .failed {
            RsLog.marker("SESSVIEW", ["renderer": "flush", "failed": true])
            renderer.flush()
            return false
        }
        if renderer.requiresFlushToResumeDecoding {
            renderer.flush()
        }
        renderer.enqueue(sample)
        return renderer.status != .failed
    }

    /// flush() keeps the displayed image. Never use removingDisplayedImage on
    /// epoch changes or closure; the last remote frame stays visible.
    func flush() { renderer.flush() }
}
