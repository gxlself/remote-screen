import Foundation

public protocol LifecycleExitMarkerStorage {
    func bool(forKey key: String) -> Bool
    func set(_ value: Bool, forKey key: String)
}

extension UserDefaults: LifecycleExitMarkerStorage {}

/// Tracks whether the previous process ended after entering the background.
public final class LifecycleExitMarker {
    public static let key = "rs.lifecycle.inBackground.v1"

    private let storage: LifecycleExitMarkerStorage
    private let storageKey: String

    public init(
        storage: LifecycleExitMarkerStorage,
        key: String = LifecycleExitMarker.key
    ) {
        self.storage = storage
        self.storageKey = key
    }

    /// Reads and clears the marker for the current launch.
    @discardableResult
    public func start() -> Bool {
        let previous = storage.bool(forKey: storageKey)
        storage.set(false, forKey: storageKey)
        return previous
    }

    public func didEnterBackground() {
        storage.set(true, forKey: storageKey)
    }

    public func willEnterForeground() {
        storage.set(false, forKey: storageKey)
    }

    public func willTerminate() {
        storage.set(false, forKey: storageKey)
    }

    // Explicit aliases keep the marker easy to use from lifecycle adapters.
    public func markBackground() { didEnterBackground() }
    public func markForeground() { willEnterForeground() }
    public func markTerminated() { willTerminate() }
}

/// Small in-memory store useful to platform-neutral tests and previews.
public final class InMemoryLifecycleExitMarkerStorage: LifecycleExitMarkerStorage {
    private var values: [String: Bool] = [:]

    public init() {}

    public func bool(forKey key: String) -> Bool {
        values[key] ?? false
    }

    public func set(_ value: Bool, forKey key: String) {
        values[key] = value
    }
}
