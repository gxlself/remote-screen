/// Command shortcuts owned by the remote computer. Inputs describe US ANSI
/// key positions; modifier masks use the same left/right bits as wire HID.
public enum KeyboardPassthrough {
    public struct Combo: Equatable, Hashable, Sendable {
        public let input: String
        public let usage: UInt16
        public let mods: UInt8

        public init(input: String, usage: UInt16, mods: UInt8) {
            self.input = input
            self.usage = usage
            self.mods = mods
        }
    }

    public enum Reserved: Equatable, Sendable {
        case shortcut(Combo)
        // Globe/Fn is a UIKit flag, not a bit in the eight-bit HID modifier
        // mask. Native callers pass that flag separately to isReserved.
        case globeCombinations
    }

    public struct KeyEvent: Equatable, Sendable {
        public let usage: UInt16
        public let down: Bool
        public let mods: UInt8
    }

    public static let commandCombos: [Combo] = {
        let letters = zip("abcdefghijklmnopqrstuvwxyz", UInt16(0x04)...0x1D)
        let digits = zip("1234567890", UInt16(0x1E)...0x27)
        let punctuation = zip("-=[]\\;'`,./", [UInt16(0x2D), 0x2E, 0x2F, 0x30, 0x31,
                                                  0x33, 0x34, 0x35, 0x36, 0x37, 0x38])
        return (Array(letters) + Array(digits) + Array(punctuation)).flatMap { input, usage in
            [Combo(input: String(input), usage: usage, mods: 0x08),
             Combo(input: String(input), usage: usage, mods: 0x0A)]
        }
    }()

    /// iPadOS can consume these before the responder chain. Toolbar combo
    /// buttons remain the reliable way to send them to the remote computer.
    public static let systemReserved: [Reserved] = [
        .shortcut(Combo(input: "\t", usage: 0x2B, mods: 0x08)), // Command-Tab
        .shortcut(Combo(input: "\t", usage: 0x2B, mods: 0x0A)), // Shift-Command-Tab
        .shortcut(Combo(input: " ", usage: 0x2C, mods: 0x08)), // Spotlight
        .shortcut(Combo(input: "h", usage: 0x0B, mods: 0x08)), // Home
        .shortcut(Combo(input: "3", usage: 0x20, mods: 0x0A)), // Screenshots
        .shortcut(Combo(input: "4", usage: 0x21, mods: 0x0A)),
        .shortcut(Combo(input: "5", usage: 0x22, mods: 0x0A)),
        .shortcut(Combo(input: "d", usage: 0x07, mods: 0x0C)), // Dock
        .globeCombinations,
    ]

    /// Paste. With clipboard sync on, JS pushes the clipboard and then sends it.
    public static let commandV = Combo(input: "v", usage: 0x19, mods: 0x08)

    /// Command-period stands in for Escape on iPad keyboards that have none
    /// (Magic Keyboard). Shift-Command-period still goes through as itself.
    public static let commandPeriod = Combo(input: ".", usage: 0x37, mods: 0x08)

    public static func commandCombo(usage: UInt16, mods: UInt8) -> Combo? {
        let normalized = normalizedModifiers(mods)
        return commandCombos.first { $0.usage == usage && $0.mods == normalized }
    }

    public static func isReserved(usage: UInt16, mods: UInt8) -> Bool {
        let normalized = normalizedModifiers(mods)
        return systemReserved.contains { reserved in
            guard case let .shortcut(combo) = reserved else { return false }
            return combo.usage == usage && combo.mods == normalized
        }
    }

    public static func isReserved(usage: UInt16, mods: UInt8, globe: Bool) -> Bool {
        globe || isReserved(usage: usage, mods: mods)
    }

    public static func keyEvents(for combo: Combo, heldModifiers: UInt8,
                                 commandPeriodAsEscape: Bool) -> [KeyEvent] {
        guard commandPeriodAsEscape, combo == commandPeriod else {
            return keyEvents(for: combo, heldModifiers: heldModifiers)
        }
        return escapeEvents(heldModifiers: heldModifiers)
    }

    /// Escape on its own: a held Command is lifted around it and pressed
    /// again afterwards, so its physical key-up still matches.
    public static func escapeEvents(heldModifiers: UInt8) -> [KeyEvent] {
        let sides: [(usage: UInt16, bit: UInt8)] = [(0xE3, 0x08), (0xE7, 0x80)]
        let commands = sides.filter { heldModifiers & $0.bit != 0 }
        var mods = heldModifiers
        var events: [KeyEvent] = []
        for command in commands {
            mods &= ~command.bit
            events.append(KeyEvent(usage: command.usage, down: false, mods: mods))
        }
        events.append(KeyEvent(usage: 0x29, down: true, mods: mods))
        events.append(KeyEvent(usage: 0x29, down: false, mods: mods))
        for command in commands.reversed() {
            mods |= command.bit
            events.append(KeyEvent(usage: command.usage, down: true, mods: mods))
        }
        return events
    }

    /// UIKeyCommand may arrive with or without physical modifier presses.
    /// Send a complete shortcut, then restore any physically held Command so
    /// a following raw key or the eventual physical key-up remains correct.
    public static func keyEvents(for combo: Combo, heldModifiers: UInt8) -> [KeyEvent] {
        let commandUsage: UInt16 = heldModifiers & 0x80 != 0 ? 0xE7 : 0xE3
        let commandBit: UInt8 = commandUsage == 0xE7 ? 0x80 : 0x08
        let addShift = combo.mods & 0x02 != 0 && heldModifiers & 0x22 == 0
        var mods = heldModifiers
        var events: [KeyEvent] = []
        if addShift {
            mods |= 0x02
            events.append(KeyEvent(usage: 0xE1, down: true, mods: mods))
        }
        mods |= commandBit
        events.append(KeyEvent(usage: commandUsage, down: true, mods: mods))
        events.append(KeyEvent(usage: combo.usage, down: true, mods: mods))
        events.append(KeyEvent(usage: combo.usage, down: false, mods: mods))
        mods &= ~commandBit
        events.append(KeyEvent(usage: commandUsage, down: false, mods: mods))
        if addShift {
            mods &= ~0x02
            events.append(KeyEvent(usage: 0xE1, down: false, mods: mods))
        }
        if heldModifiers & commandBit != 0 {
            events.append(KeyEvent(usage: commandUsage, down: true, mods: heldModifiers))
        }
        return events
    }

    private static func normalizedModifiers(_ mods: UInt8) -> UInt8 {
        (mods & 0x0F) | (mods >> 4)
    }
}

/// Control-Option-Command shortcuts that switch between open sessions or show
/// and hide the toolbar. They stay on this device: the capture view hands them
/// to JS instead of sending the key to the remote computer.
public enum SessionShortcut: Equatable, Sendable {
    case next
    case previous
    /// The Nth session in the switcher, 1...9.
    case index(Int)
    case switcher
    case toolbar

    /// The `action` field of the JS event.
    public var action: String {
        switch self {
        case .next: return "next"
        case .previous: return "previous"
        case .index: return "index"
        case .switcher: return "switcher"
        case .toolbar: return "toolbar"
        }
    }

    /// The `index` field of the JS event; 0 unless `.index`.
    public var index: Int {
        if case let .index(value) = self { return value }
        return 0
    }
}

public extension KeyboardPassthrough {
    /// Control + Option + Command, either side, nothing else.
    static let sessionShortcutMods: UInt8 = 0x0D

    /// ⌃⌥⌘→ / ⌃⌥⌘← next / previous, ⌃⌥⌘1…9 the Nth session, ⌃⌥⌘S the switcher,
    /// ⌃⌥⌘T the toolbar.
    static func sessionShortcut(usage: UInt16, mods: UInt8) -> SessionShortcut? {
        guard (mods & 0x0F) | (mods >> 4) == sessionShortcutMods else { return nil }
        switch usage {
        case 0x4F: return .next
        case 0x50: return .previous
        case 0x1E...0x26: return .index(Int(usage - 0x1E) + 1)
        case 0x16: return .switcher
        case 0x17: return .toolbar
        default: return nil
        }
    }
}
