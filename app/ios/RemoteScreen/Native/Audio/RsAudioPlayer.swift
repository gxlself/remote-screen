import AVFoundation
import Darwin
import UIKit

@objc(RsAudioPlayer)
final class RsAudioPlayer: NSObject {
    @objc(shared)
    static let shared = RsAudioPlayer()

    private enum PlaybackError: Error {
        case openNull
        case invalidFormat
        case missingHandle
    }

    private let audioSession = RsAudioSession()
    private var engine = AVAudioEngine()
    private var sourceNode: AVAudioSourceNode?
    private var engineObserver: NSObjectProtocol?
    private var handle: OpaquePointer?
    private var muted = false
    private var machine = AudioPlaybackMachine()

    // OSAtomic keeps the slot available on iOS 17 without adding a dependency
    // or requiring the iOS 18 Synchronization module. Only main changes bits;
    // the render thread performs an atomic add of zero to load them.
    private let renderSlot: UnsafeMutablePointer<Int64>

    private override init() {
        let slot = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
        slot.initialize(to: 0)
        renderSlot = slot
        super.init()
        audioSession.eventHandler = { [weak self] event in
            self?.handleSessionEvent(event)
        }
        observeEngineConfiguration()
    }

    deinit {
        if let engineObserver {
            NotificationCenter.default.removeObserver(engineObserver)
        }
        closeAudio()
        renderSlot.deinitialize(count: 1)
        renderSlot.deallocate()
    }

    @objc(start:)
    func start(_ value: String) -> Bool {
        onMain { [self] in
            guard SessionIdParse.parse(value) != nil else {
                RsLog.marker("AUDIO", ["open": "null"])
                return false
            }
            // A JS start may already be queued when the app enters background.
            // Observe UIKit even if its notification is pending.
            if UIApplication.shared.applicationState == .background {
                _ = process(.enterBackground)
            }
            return process(.start(value)) && handle != nil
        }
    }

    @objc(stop)
    func stop() {
        onMain { [self] in _ = process(.stop) }
    }

    @objc(setMuted:)
    func setMuted(_ value: Bool) {
        onMain { [self] in
            muted = value
            if let handle { rs_audio_set_muted(handle, value) }
        }
    }

    @objc(stats)
    func stats() -> NSDictionary {
        onMain { [self] in
            var native = RsAudioStats()
            if let handle { rs_audio_stats(handle, &native) }
            return [
                "running": engine.isRunning && machine.state.name == "running",
                "packets": Double(native.packets),
                "lost": Double(native.lost),
                "concealed": Double(native.concealed),
                "underruns": Double(native.underruns),
                "targetMs": Double(native.target_ms),
                "levelMs": Double(native.level_ms),
                "outputLatencyMs": audioSession.outputLatencyMs,
                "ioBufferMs": audioSession.ioBufferMs,
                "route": audioSession.routeName,
            ] as NSDictionary
        }
    }

    private func onMain<T>(_ action: () -> T) -> T {
        if Thread.isMainThread { return action() }
        return DispatchQueue.main.sync(execute: action)
    }

    private func handleSessionEvent(_ event: RsAudioSessionEvent) {
        switch event {
        case .interruptionBegan:
            _ = process(.interruptionBegan)
        case .interruptionEnded(let shouldResume):
            _ = process(.interruptionEnded(shouldResume: shouldResume))
        case .routeChanged:
            // oldDeviceUnavailable keeps the existing mute setting and follows
            // the new route, including the speaker.
            if process(.configurationChanged) {
                emit(state: machine.state, reason: "route")
            }
        case .mediaServicesReset:
            if process(.mediaServicesReset) {
                emit(state: machine.state, reason: "mediaServicesReset")
            }
        case .enterBackground:
            _ = process(.enterBackground)
        case .enterForeground:
            _ = process(.enterForeground)
        }
    }

    private func observeEngineConfiguration() {
        engineObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange,
            object: engine,
            queue: .main
        ) { [weak self] note in
            guard let changedEngine = note.object as? AVAudioEngine else { return }
            // Graph changes may post synchronously. Ignore replaced engines
            // and finish the current transition before trying another start.
            DispatchQueue.main.async { [weak self, weak changedEngine] in
                guard let self, let changedEngine, self.engine === changedEngine else { return }
                _ = self.process(.configurationChanged)
            }
        }
    }

    private func process(_ event: AudioPlaybackEvent) -> Bool {
        let actions = machine.handle(event)
        do {
            for action in actions { try execute(action) }
            return true
        } catch {
            let reason: String
            switch error {
            case PlaybackError.openNull: reason = "open-null"
            case PlaybackError.invalidFormat: reason = "invalid-format"
            case PlaybackError.missingHandle: reason = "missing-handle"
            default:
                let nativeError = error as NSError
                reason = "audio:\(nativeError.domain):\(nativeError.code)"
            }
            let cleanup = machine.handle(.engineStartFailed(reason))
            for action in cleanup { try? execute(action) }
            return false
        }
    }

    private func execute(_ action: AudioPlaybackAction) throws {
        switch action {
        case .activateSession:
            try audioSession.activate()
        case .deactivateSession:
            audioSession.deactivate()
        case .openAudio(let value):
            guard let id = SessionIdParse.parse(value), let opened = rs_audio_open(id) else {
                RsLog.marker("AUDIO", ["open": "null"])
                throw PlaybackError.openNull
            }
            handle = opened
            rs_audio_set_muted(opened, muted)
        case .closeAudio:
            closeAudio()
        case .startEngine:
            try startEngine()
        case .stopEngine:
            stopEngine()
        case .rebuildEngine:
            rebuildEngine()
        case .emit(let state, let reason):
            emit(state: state, reason: reason)
        }
    }

    private func startEngine() throws {
        guard let handle else { throw PlaybackError.missingHandle }
        guard !engine.isRunning else { return }
        if sourceNode == nil { try installSourceNode() }
        publish(handle)
        engine.prepare()
        try engine.start()
    }

    private func stopEngine() {
        // Stop also supplies the render barrier after a system configuration
        // change. The handle is released only after stopping and detaching.
        engine.stop()
        publish(nil)
    }

    private func closeAudio() {
        stopEngine()
        if let sourceNode { engine.detach(sourceNode) }
        sourceNode = nil
        if let handle { rs_audio_close(handle) }
        handle = nil
    }

    private func rebuildEngine() {
        stopEngine()
        if let engineObserver {
            NotificationCenter.default.removeObserver(engineObserver)
        }
        if let sourceNode { engine.detach(sourceNode) }
        sourceNode = nil
        engine = AVAudioEngine()
        observeEngineConfiguration()
    }

    private func publish(_ value: OpaquePointer?) {
        assert(Thread.isMainThread, "Audio handles are published on the main thread")
        let next = value.map { Int64(Int(bitPattern: $0)) } ?? 0
        let previous = OSAtomicAdd64Barrier(0, renderSlot)
        let exchanged = OSAtomicCompareAndSwap64Barrier(previous, next, renderSlot)
        assert(exchanged, "Only the main thread changes the audio handle slot")
    }

    private func installSourceNode() throws {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2) else {
            throw PlaybackError.invalidFormat
        }
        let slot = renderSlot
        let source = AVAudioSourceNode(format: format) { isSilence, _, frameCount, output in
            // Only preallocated raw storage is captured by this callback.
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            let bits = OSAtomicAdd64Barrier(0, slot)
            let byteCount = Int(frameCount) * MemoryLayout<Float>.size
            if let audio = OpaquePointer(bitPattern: Int(bits)), buffers.count == 2,
               buffers[0].mNumberChannels == 1, buffers[1].mNumberChannels == 1,
               Int(buffers[0].mDataByteSize) >= byteCount,
               Int(buffers[1].mDataByteSize) >= byteCount,
               let left = buffers[0].mData, let right = buffers[1].mData {
                _ = rs_audio_render(audio, left.assumingMemoryBound(to: Float.self),
                                    right.assumingMemoryBound(to: Float.self), frameCount)
                isSilence.pointee = false
            } else {
                for buffer in buffers {
                    if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) }
                }
                isSilence.pointee = true
            }
            return noErr
        }
        sourceNode = source
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
    }

    private func emit(state: AudioPlaybackState, reason: String) {
        RsLog.marker("AUDIO", [
            "state": state.name,
            "reason": reason,
            "route": audioSession.routeName,
            "ioBufferMs": audioSession.ioBufferMs,
            "outputLatencyMs": audioSession.outputLatencyMs,
        ])
        NotificationCenter.default.post(
            name: Notification.Name("RsAudioStateDidChange"),
            object: self,
            userInfo: ["state": state.name, "reason": reason]
        )
    }
}
