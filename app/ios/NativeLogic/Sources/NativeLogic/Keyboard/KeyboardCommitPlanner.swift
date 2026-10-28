enum KeyboardInputMode: String {
    case text
    case keys
}

enum KeyboardAction: Equatable {
    case text(String)
    case key(usage: UInt16, down: Bool, mods: UInt8)
    case stickyConsumed(UInt8)
}

enum KeyboardCommitPlanner {
    static func plan(
        committed: String,
        mode: KeyboardInputMode,
        stickyMods: UInt8
    ) -> [KeyboardAction] {
        var actions: [KeyboardAction] = []
        var remaining = committed[...]
        var pendingText = ""

        func flushText() {
            actions.append(contentsOf: KeyboardTextSplitter.split(pendingText).map(KeyboardAction.text))
            pendingText = ""
        }

        func appendKey(usage: UInt16, mods: UInt8) {
            actions.append(.key(usage: usage, down: true, mods: mods))
            actions.append(.key(usage: usage, down: false, mods: mods))
        }

        // Only the first character of this commit can consume the toolbar's
        // sticky mask. A non-ANSI first character leaves it armed for later.
        if stickyMods != 0, let first = remaining.first,
           let mapped = KeyboardHidMap.usage(for: first) {
            appendKey(usage: mapped.usage, mods: stickyMods | (mapped.shift ? 0x02 : 0))
            actions.append(.stickyConsumed(stickyMods))
            remaining = remaining.dropFirst()
        }

        for character in remaining {
            if mode == .keys, let mapped = KeyboardHidMap.usage(for: character) {
                flushText()
                appendKey(usage: mapped.usage, mods: mapped.shift ? 0x02 : 0)
            } else if character == "\n" {
                flushText()
                appendKey(usage: 0x28, mods: 0)
            } else {
                pendingText.append(character)
            }
        }
        flushText()
        return actions
    }
}
