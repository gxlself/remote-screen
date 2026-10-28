import CoreGraphics
import Foundation

/// Which sessions keep their last picture for the three-finger swipe
///: at most `capacity`, the least recently used goes first.
/// Only the bookkeeping lives here; the app holds the images by key.
public struct SessionSnapshotLRU<Key: Hashable> {
    public static var defaultCapacity: Int { 6 }
    /// Snapshots are scaled down to at most this many pixels wide.
    public static var maxWidth: Int { 1920 }

    public let capacity: Int
    /// Oldest first.
    public private(set) var order: [Key] = []

    public init(capacity: Int = SessionSnapshotLRU.defaultCapacity) {
        self.capacity = max(1, capacity)
    }

    public func contains(_ key: Key) -> Bool { order.contains(key) }

    /// Stores or refreshes `key`; returns the keys pushed out.
    public mutating func insert(_ key: Key) -> [Key] {
        order.removeAll { $0 == key }
        order.append(key)
        guard order.count > capacity else { return [] }
        let evicted = Array(order.prefix(order.count - capacity))
        order.removeFirst(order.count - capacity)
        return evicted
    }

    /// Marks `key` as just used (shown beside the picture).
    public mutating func touch(_ key: Key) {
        guard let index = order.firstIndex(of: key) else { return }
        order.remove(at: index)
        order.append(key)
    }

    /// Forgets `key`; true when it was there.
    @discardableResult
    public mutating func remove(_ key: Key) -> Bool {
        let before = order.count
        order.removeAll { $0 == key }
        return order.count != before
    }

    /// Keeps only `live` (sessions still open); returns the keys dropped.
    public mutating func retain(only live: Set<Key>) -> [Key] {
        let dropped = order.filter { !live.contains($0) }
        order.removeAll { !live.contains($0) }
        return dropped
    }

    /// The pixel size a `width`×`height` picture is stored at: no wider than
    /// `limit` (the screen, at most `maxWidth`), aspect kept, never enlarged.
    public static func scaledSize(width: Int, height: Int, limit: Int) -> (width: Int, height: Int) {
        guard width > 0, height > 0 else { return (0, 0) }
        let cap = max(1, min(limit, maxWidth))
        guard width > cap else { return (width, height) }
        let scaled = Int((Double(height) * Double(cap) / Double(width)).rounded())
        return (cap, max(1, scaled))
    }
}
