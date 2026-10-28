import Foundation

/// a computer in a system window of its own. The JS side names the
/// window and describes its session; the scene keeps both in its activity
/// and session user info, so iPadOS can bring the window back later.
struct WindowRequest: Equatable {
    let windowId: String
    /// What the window shows, opaque here (JS windowTarget.ts).
    let payload: String
    /// The scene title in App Exposé.
    let title: String
}

enum WindowActivity {
    static let type = "ai.gxlself.remotescreen.window"
    /// The window with the whole app; never opened by a request.
    static let mainWindowId = "main"
    static let windowIdKey = "windowId"
    static let payloadKey = "payload"
    static let titleKey = "title"
    static let maxWindowIdLength = 64
    static let maxPayloadBytes = 8 * 1024
    static let maxTitleLength = 200
    /// A window iPadOS opened by itself while the main one is up: it starts
    /// on the computer list (JS windowTarget.ts `PICKER_PAYLOAD`).
    static let pickerPayload = #"{"kind":"picker"}"#
    static let pickerPrefix = "p"

    /// A 选电脑 window named from `random` (any unique token, e.g. a UUID).
    static func pickerRequest(random: String) -> WindowRequest {
        let token = random.unicodeScalars
            .filter { $0.isASCII && CharacterSet.alphanumerics.contains($0) }
            .map(String.init).joined()
        let windowId = String((pickerPrefix + token).prefix(maxWindowIdLength))
        return WindowRequest(windowId: windowId, payload: pickerPayload, title: "")
    }

    static func isPicker(_ request: WindowRequest) -> Bool {
        request.payload == pickerPayload
    }

    static func userInfo(_ request: WindowRequest) -> [String: String] {
        [
            windowIdKey: request.windowId,
            payloadKey: request.payload,
            titleKey: request.title,
        ]
    }

    static func isValidWindowId(_ windowId: String) -> Bool {
        guard !windowId.isEmpty, windowId != mainWindowId,
              windowId.count <= maxWindowIdLength else { return false }
        return windowId.unicodeScalars.allSatisfy { scalar in
            scalar.isASCII && (CharacterSet.alphanumerics.contains(scalar) || scalar == "-" || scalar == "_")
        }
    }

    static func isValid(_ request: WindowRequest) -> Bool {
        isValidWindowId(request.windowId) && !request.payload.isEmpty
            && request.payload.utf8.count <= maxPayloadBytes
    }

    /// The request a scene was opened or restored with; nil for the main window.
    static func decode(_ userInfo: [AnyHashable: Any]?) -> WindowRequest? {
        guard let userInfo,
              let windowId = userInfo[windowIdKey] as? String,
              let payload = userInfo[payloadKey] as? String else { return nil }
        let title = String((userInfo[titleKey] as? String ?? "").prefix(maxTitleLength))
        let request = WindowRequest(windowId: windowId, payload: payload, title: title)
        return isValid(request) ? request : nil
    }

    /// The request a scene session kept (`UISceneSession.userInfo`).
    static func decode(sessionInfo: [String: Any]?) -> WindowRequest? {
        guard let sessionInfo else { return nil }
        var userInfo: [AnyHashable: Any] = [:]
        for (key, value) in sessionInfo {
            userInfo[AnyHashable(key)] = value
        }
        return decode(userInfo)
    }
}

/// The scene state JS sees per window.
enum WindowPhase: String {
    case active
    case inactive
    case background
    /// The scene session is gone for good.
    case closed
}

/// Which scene session holds which window. Sessions are their persistent
/// identifiers; a session holds one window and a window one session.
struct WindowBook: Equatable {
    private(set) var sessions: [String: String] = [:]

    mutating func connect(windowId: String, session: String) {
        for (other, held) in sessions where held == session && other != windowId {
            sessions.removeValue(forKey: other)
        }
        sessions[windowId] = session
    }

    func session(for windowId: String) -> String? {
        sessions[windowId]
    }

    func windowId(for session: String) -> String? {
        sessions.first { $0.value == session }?.key
    }

    /// Forgets the discarded sessions; returns their windows, sorted.
    mutating func discard(_ discarded: Set<String>) -> [String] {
        let gone = sessions.filter { discarded.contains($0.value) }.map(\.key).sorted()
        for windowId in gone {
            sessions.removeValue(forKey: windowId)
        }
        return gone
    }

    /// A plain new scene (App Exposé's +) while the main window still has a
    /// connected scene of its own: the app keeps one main window.
    func isDuplicateMain(session: String, connected: Set<String>) -> Bool {
        guard let main = sessions[WindowActivity.mainWindowId], main != session else {
            return false
        }
        return connected.contains(main)
    }
}
