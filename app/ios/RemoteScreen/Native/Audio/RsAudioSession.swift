import AVFoundation
import UIKit

enum RsAudioSessionEvent {
    case interruptionBegan
    case interruptionEnded(shouldResume: Bool)
    case routeChanged
    case mediaServicesReset
    case enterBackground
    case enterForeground
}

/// Owns the AVAudioSession policy and translates system notifications into
/// events consumed by RsAudioPlayer.
@objc(RsAudioSession)
final class RsAudioSession: NSObject {
    var eventHandler: ((RsAudioSessionEvent) -> Void)?

    private var audioSession: AVAudioSession { AVAudioSession.sharedInstance() }
    private var observers: [NSObjectProtocol] = []
    private var observing = false

    override init() {
        super.init()
        startObserving()
    }

    deinit {
        stopObserving()
    }

    func activate() throws {
        try audioSession.setCategory(.playback, mode: .default, options: [.mixWithOthers])
        try audioSession.setPreferredSampleRate(48_000)
        try audioSession.setPreferredIOBufferDuration(0.005)
        try audioSession.setActive(true)
    }

    func deactivate() {
        try? audioSession.setActive(false, options: [.notifyOthersOnDeactivation])
    }

    var outputLatencyMs: Double {
        max(0, audioSession.outputLatency * 1000)
    }

    var ioBufferMs: Double {
        max(0, audioSession.ioBufferDuration * 1000)
    }

    var routeName: String {
        let names = audioSession.currentRoute.outputs.map(\.portName)
        return names.isEmpty ? "none" : names.joined(separator: ",")
    }

    private func startObserving() {
        guard !observing else { return }
        observing = true
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: nil,
            queue: .main
        ) { [weak self] note in
            self?.handleInterruption(note)
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.deliver(.routeChanged)
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.mediaServicesWereResetNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.deliver(.mediaServicesReset)
        })
        observers.append(center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.deliver(.enterBackground)
        })
        observers.append(center.addObserver(
            forName: UIApplication.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.deliver(.enterForeground)
        })
    }

    private func stopObserving() {
        let center = NotificationCenter.default
        for observer in observers { center.removeObserver(observer) }
        observers.removeAll()
        observing = false
    }

    private func handleInterruption(_ note: Notification) {
        guard let rawType = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? NSNumber)?.uintValue,
              let type = AVAudioSession.InterruptionType(rawValue: rawType) else {
            return
        }
        switch type {
        case .began:
            deliver(.interruptionBegan)
        case .ended:
            let rawOptions = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? NSNumber)?.uintValue ?? 0
            let options = AVAudioSession.InterruptionOptions(rawValue: rawOptions)
            deliver(.interruptionEnded(shouldResume: options.contains(.shouldResume)))
        @unknown default:
            break
        }
    }

    private func deliver(_ event: RsAudioSessionEvent) {
        // setActive can post notifications synchronously while a start action
        // is still opening the handle. Finish that action list before reacting.
        DispatchQueue.main.async { [weak self] in
            self?.eventHandler?(event)
        }
    }
}
