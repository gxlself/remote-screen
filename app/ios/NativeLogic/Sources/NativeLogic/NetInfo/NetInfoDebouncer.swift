import Foundation

/// A clock-free trailing-edge debouncer. Callers provide monotonically
/// increasing microsecond timestamps, which keeps the logic deterministic in
/// SwiftPM tests and lets the iOS monitor use DispatchTime.
struct NetInfoDebouncer {
    let delay: UInt64
    private(set) var deadline: UInt64?

    init(delay: UInt64 = 500_000) {
        self.delay = delay
        self.deadline = nil
    }

    mutating func update(at timestamp: UInt64) {
        deadline = timestamp &+ delay
    }

    mutating func fireIfDue(at timestamp: UInt64, _ action: () -> Void) {
        guard let deadline, timestamp >= deadline else { return }
        self.deadline = nil
        action()
    }

    func isDue(at timestamp: UInt64) -> Bool {
        guard let deadline else { return false }
        return timestamp >= deadline
    }
}
