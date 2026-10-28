import Foundation
import UIKit

/// the system windows (scenes) of the app, named for JS. `main` is
/// the window with the whole app; each other window shows one computer and
/// carries a `WindowRequest` in its activity and scene session user info.
@objc(RsWindowCoordinator)
final class RsWindowCoordinator: NSObject {
    typealias StateHandler = (NSDictionary) -> Void

    @objc(shared)
    static let shared = RsWindowCoordinator()

    private var book = WindowBook()
    private var phases: [String: WindowPhase] = [:]
    private var listener: StateHandler?

    private override init() {
        super.init()
    }

    /// Several windows: an iPad, with `UIApplicationSupportsMultipleScenes`.
    /// Fixed for the process; the first scene reads it on the main thread,
    /// before JS asks (from its own thread) through the module's constants.
    @objc(supported)
    let supported: Bool = UIDevice.current.userInterfaceIdiom == .pad && RsWindowCoordinator.manifestAllowsScenes()

    private static func manifestAllowsScenes() -> Bool {
        let manifest = Bundle.main.object(forInfoDictionaryKey: "UIApplicationSceneManifest") as? [String: Any]
        return manifest?["UIApplicationSupportsMultipleScenes"] as? Bool ?? false
    }

    @objc(registerWithStateCallback:)
    func register(withStateCallback callback: @escaping StateHandler) {
        DispatchQueue.main.async { [weak self] in
            self?.listener = callback
            self?.emitAll()
        }
    }

    @objc(refresh)
    func refresh() {
        DispatchQueue.main.async { [weak self] in self?.emitAll() }
    }

    @objc(open:payload:title:)
    func open(_ windowId: String, payload: String, title: String) {
        DispatchQueue.main.async { [weak self] in
            self?.openOnMain(WindowRequest(windowId: windowId, payload: payload, title: title))
        }
    }

    @objc(activate:)
    func activate(_ windowId: String) {
        DispatchQueue.main.async { [weak self] in self?.activateOnMain(windowId) }
    }

    @objc(close:)
    func close(_ windowId: String) {
        DispatchQueue.main.async { [weak self] in self?.closeOnMain(windowId) }
    }

    // MARK: - Scenes (SceneDelegate / AppDelegate, main thread)

    func sceneConnected(_ windowId: String, session: UISceneSession) {
        book.connect(windowId: windowId, session: session.persistentIdentifier)
    }

    func sceneChanged(_ windowId: String, phase: WindowPhase) {
        guard phases[windowId] != phase else { return }
        phases[windowId] = phase
        emit(windowId, phase)
    }

    func scenesDiscarded(_ sessions: Set<UISceneSession>) {
        let gone = book.discard(Set(sessions.map(\.persistentIdentifier)))
        for windowId in gone {
            if windowId != WindowActivity.mainWindowId {
                RsReactRuntime.shared.dropWindow(windowId)
            }
            phases.removeValue(forKey: windowId)
            emit(windowId, .closed)
        }
    }

    /// A plain new scene while the main window has one already.
    func isDuplicateMain(_ session: UISceneSession) -> Bool {
        let connected = Set(UIApplication.shared.connectedScenes.map(\.session.persistentIdentifier))
        return book.isDuplicateMain(session: session.persistentIdentifier, connected: connected)
    }

    static func activity(for request: WindowRequest) -> NSUserActivity {
        let activity = NSUserActivity(activityType: WindowActivity.type)
        activity.title = request.title
        activity.userInfo = WindowActivity.userInfo(request)
        activity.isEligibleForHandoff = false
        activity.isEligibleForSearch = false
        activity.isEligibleForPrediction = false
        return activity
    }

    // MARK: - Private

    private func openOnMain(_ request: WindowRequest) {
        guard WindowActivity.isValid(request), supported else {
            emit(request.windowId, .closed)
            return
        }
        if let session = sceneSession(for: request.windowId) {
            requestActivation(session, activity: nil, windowId: request.windowId)
            return
        }
        // iOS 16 API on purpose: the deployment target is 16, and iOS 17's
        // UISceneSessionActivationRequest adds nothing needed here.
        requestActivation(nil, activity: Self.activity(for: request), windowId: request.windowId)
    }

    private func activateOnMain(_ windowId: String) {
        guard supported else { return }
        if let session = sceneSession(for: windowId) {
            requestActivation(session, activity: nil, windowId: windowId)
        } else if windowId == WindowActivity.mainWindowId {
            // The main window was closed in App Exposé: a plain scene is main.
            requestActivation(nil, activity: nil, windowId: windowId)
        }
    }

    private func closeOnMain(_ windowId: String) {
        guard windowId != WindowActivity.mainWindowId,
              let session = sceneSession(for: windowId) else {
            if windowId != WindowActivity.mainWindowId, phases[windowId] != nil {
                phases.removeValue(forKey: windowId)
                emit(windowId, .closed)
            }
            return
        }
        UIApplication.shared.requestSceneSessionDestruction(session, options: nil) { error in
            RsLog.marker("WINDOW", ["close": "error", "message": error.localizedDescription])
        }
    }

    private func requestActivation(_ session: UISceneSession?, activity: NSUserActivity?, windowId: String) {
        UIApplication.shared.requestSceneSessionActivation(
            session,
            userActivity: activity,
            options: nil
        ) { [weak self] error in
            RsLog.marker("WINDOW", ["activate": "error", "message": error.localizedDescription])
            DispatchQueue.main.async {
                guard let self, activity != nil, self.sceneSession(for: windowId) == nil else { return }
                self.emit(windowId, .closed)
            }
        }
    }

    private func sceneSession(for windowId: String) -> UISceneSession? {
        guard let identifier = book.session(for: windowId) else { return nil }
        return UIApplication.shared.openSessions.first { $0.persistentIdentifier == identifier }
    }

    private func emitAll() {
        for (windowId, phase) in phases.sorted(by: { $0.key < $1.key }) {
            emit(windowId, phase)
        }
    }

    private func emit(_ windowId: String, _ phase: WindowPhase) {
        listener?(["windowId": windowId, "state": phase.rawValue] as NSDictionary)
    }
}
