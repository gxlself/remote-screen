import GameController
import Foundation

/// Connection state only; UIPress/UIKeyCommand remain the input event path.
final class HardwareKeyboardMonitor {
    private(set) var isConnected = GCKeyboard.coalesced != nil
    var onChange: ((Bool) -> Void)?

    private var observers: [NSObjectProtocol] = []

    init() {
        for name in [Notification.Name.GCKeyboardDidConnect, .GCKeyboardDidDisconnect] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil,
                                                                    queue: .main) { [weak self] _ in
                // The coalesced keyboard changes with the connection event.
                // Refresh on the next main turn to also cover disconnection.
                DispatchQueue.main.async { [weak self] in self?.refresh() }
            })
        }
    }

    func refresh() {
        let connected = GCKeyboard.coalesced != nil
        guard connected != isConnected else { return }
        isConnected = connected
        onChange?(connected)
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }
}
