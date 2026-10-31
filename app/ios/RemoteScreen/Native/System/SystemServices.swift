import Foundation
import Darwin
import UIKit
import UserNotifications

@objc(SystemServices)
final class SystemServices: NSObject {
    @objc(shared)
    static let shared = SystemServices()

    private var lightGenerator: UIImpactFeedbackGenerator?
    private var notificationGenerator: UINotificationFeedbackGenerator?
    private let constantsLock = NSLock()
    private var cachedConstants: NSDictionary?

    private override init() {
        super.init()
    }

    @objc(getConstants)
    func getConstants() -> NSDictionary {
        constantsLock.lock()
        let constants = cachedConstants
        constantsLock.unlock()
        if let constants {
            return constants
        }

        // UIScreen is main-actor isolated. This is only a defensive fallback
        // for a caller that reaches constants before module initialization.
        return performOnMainSync {
            constantsOnMainThread()
        }
    }

    @objc(prepareConstants)
    func prepareConstants() {
        performOnMainSync {
            _ = constantsOnMainThread()
        }
    }

    private func constantsOnMainThread() -> NSDictionary {
        constantsLock.lock()
        defer { constantsLock.unlock() }
        if let cachedConstants {
            return cachedConstants
        }

        let constants: NSDictionary = [
            "appVersion": Bundle.main.object(
                forInfoDictionaryKey: "CFBundleShortVersionString"
            ) as? String ?? "0.0.0",
            "buildNumber": Bundle.main.object(
                forInfoDictionaryKey: "CFBundleVersion"
            ) as? String ?? "0",
            "bundleId": Bundle.main.bundleIdentifier ?? "",
            "modelId": modelIdentifier(),
            "maxFps": UIScreen.main.maximumFramesPerSecond,
        ]
        cachedConstants = constants
        return constants
    }

    @objc(copyText:)
    func copyText(_ text: String) {
        UIPasteboard.general.string = text
    }

    @objc(hasText)
    func hasText() -> Bool {
        UIPasteboard.general.hasStrings
    }

    /// Reading the count, unlike the contents, never asks to paste.
    @objc(clipboardChangeCount)
    func clipboardChangeCount() -> Int {
        UIPasteboard.general.changeCount
    }

    @objc(readTextWithCompletion:)
    func readText(completion: @escaping (String?) -> Void) {
        let pasteboard = UIPasteboard.general
        guard pasteboard.hasStrings else {
            completion(nil)
            return
        }
        completion(pasteboard.string)
    }

    @objc(haptic:)
    func haptic(_ rawKind: String) {
        performOnMain { [weak self] in
            guard let self, let kind = SystemHapticKind.parse(rawKind) else {
                return
            }
            switch kind {
            case .light:
                let generator: UIImpactFeedbackGenerator
                if let existing = self.lightGenerator {
                    generator = existing
                } else {
                    let created = UIImpactFeedbackGenerator(style: .light)
                    self.lightGenerator = created
                    generator = created
                }
                generator.prepare()
                generator.impactOccurred()
                generator.prepare()
            case .success:
                let generator = self.notificationGeneratorOnMain()
                generator.prepare()
                generator.notificationOccurred(.success)
                generator.prepare()
            case .warning:
                let generator = self.notificationGeneratorOnMain()
                generator.prepare()
                generator.notificationOccurred(.warning)
                generator.prepare()
            case .error:
                let generator = self.notificationGeneratorOnMain()
                generator.prepare()
                generator.notificationOccurred(.error)
                generator.prepare()
            }
        }
    }

    private func notificationGeneratorOnMain() -> UINotificationFeedbackGenerator {
        if let notificationGenerator {
            return notificationGenerator
        }
        let generator = UINotificationFeedbackGenerator()
        notificationGenerator = generator
        return generator
    }

    @objc(notificationStatusWithCompletion:)
    func notificationStatus(completion: @escaping (String) -> Void) {
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            completion(Self.notificationStatus(for: settings.authorizationStatus))
        }
    }

    @objc(requestNotificationsWithCompletion:)
    func requestNotifications(completion: @escaping (String) -> Void) {
        performOnMain {
            UNUserNotificationCenter.current().requestAuthorization(
                options: [.alert, .sound, .badge]
            ) { granted, _ in
                completion(granted ? "granted" : "denied")
            }
        }
    }

    @objc(requestLocalNetworkWithTimeoutMs:completion:)
    func requestLocalNetwork(
        timeoutMs: Double,
        completion: @escaping (String) -> Void
    ) {
        LocalNetworkProbe.shared.probe(timeoutMs: timeoutMs, completion: completion)
    }

    private static func notificationStatus(
        for status: UNAuthorizationStatus
    ) -> String {
        switch status {
        case .authorized, .provisional, .ephemeral:
            return "granted"
        case .denied:
            return "denied"
        case .notDetermined:
            return "notDetermined"
        @unknown default:
            return "denied"
        }
    }

    private func performOnMain(_ work: @escaping () -> Void) {
        if Thread.isMainThread {
            work()
        } else {
            DispatchQueue.main.async(execute: work)
        }
    }

    private func performOnMainSync<T>(_ work: () -> T) -> T {
        if Thread.isMainThread {
            return work()
        }
        return DispatchQueue.main.sync(execute: work)
    }

    private func modelIdentifier() -> String {
        if let simulator = ProcessInfo.processInfo.environment[
            "SIMULATOR_MODEL_IDENTIFIER"
        ], !simulator.isEmpty {
            return simulator
        }
        var info = utsname()
        uname(&info)
        let capacity = MemoryLayout.size(ofValue: info.machine)
        return withUnsafePointer(to: &info.machine) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: capacity) {
                String(cString: $0)
            }
        }
    }
}
