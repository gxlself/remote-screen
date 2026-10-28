import Foundation

/// Local auto-repeat for keys UIKit reports once per press (the raw route:
/// arrows, Delete, Home/End, Page Up/Down, and every key in keys mode).
/// Printable text repeats through the input system and is never fed here.
/// Only key-downs repeat; the physical key-up still ends the press.
struct KeyboardRepeat {
    enum StopReason: Equatable {
        case keyUp
        case focusLost
        case background
        case keyboardDisconnected
    }

    static let delay: TimeInterval = 0.5
    static let interval: TimeInterval = 0.033

    private(set) var usage: UInt16?
    /// When the next repeat is due; nil while nothing repeats.
    private(set) var nextFire: TimeInterval?

    /// Modifiers never repeat; lock keys would toggle on every repeat.
    static func repeats(usage: UInt16) -> Bool {
        switch usage {
        case 0xE0...0xE7, // modifiers
             0x39, // Caps Lock
             0x47, // Scroll Lock
             0x53: // Num Lock
            return false
        default:
            return true
        }
    }

    /// A raw key-down. The newest held key takes over, as on a real keyboard;
    /// a modifier-down leaves the current repeat running.
    mutating func keyDown(_ usage: UInt16, at now: TimeInterval) {
        guard Self.repeats(usage: usage) else { return }
        self.usage = usage
        nextFire = now + Self.delay
    }

    /// Releasing a key that is not the repeating one changes nothing.
    @discardableResult
    mutating func keyUp(_ usage: UInt16) -> Bool {
        guard self.usage == usage else { return false }
        return stop(.keyUp)
    }

    /// Returns whether a repeat was running.
    @discardableResult
    mutating func stop(_ reason: StopReason) -> Bool {
        guard usage != nil else { return false }
        usage = nil
        nextFire = nil
        return true
    }

    /// The key to send another down for, if one is due at `now`. A late
    /// timer sends one repeat, not a burst, and keeps the cadence from now.
    mutating func fire(at now: TimeInterval) -> UInt16? {
        guard let usage, let due = nextFire, now >= due else { return nil }
        let next = due + Self.interval
        nextFire = next > now ? next : now + Self.interval
        return usage
    }
}
