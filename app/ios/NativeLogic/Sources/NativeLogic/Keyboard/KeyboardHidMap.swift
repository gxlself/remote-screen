/// US ANSI character positions. Non-ASCII input stays on the Text path.
enum KeyboardHidMap {
    static func usage(for character: Character) -> (usage: UInt16, shift: Bool)? {
        guard character.unicodeScalars.count == 1,
              let scalar = character.unicodeScalars.first else { return nil }

        switch scalar.value {
        case 97...122: return (UInt16(scalar.value - 97 + 0x04), false)
        case 65...90: return (UInt16(scalar.value - 65 + 0x04), true)
        case 49...57: return (UInt16(scalar.value - 49 + 0x1E), false)
        case 48: return (0x27, false)
        case 32: return (0x2C, false)
        case 10: return (0x28, false)
        case 9: return (0x2B, false)
        default: return punctuation[character]
        }
    }

    private static let punctuation: [Character: (usage: UInt16, shift: Bool)] = [
        "-": (0x2D, false), "_": (0x2D, true),
        "=": (0x2E, false), "+": (0x2E, true),
        "[": (0x2F, false), "{": (0x2F, true),
        "]": (0x30, false), "}": (0x30, true),
        "\\": (0x31, false), "|": (0x31, true),
        ";": (0x33, false), ":": (0x33, true),
        "'": (0x34, false), "\"": (0x34, true),
        "`": (0x35, false), "~": (0x35, true),
        ",": (0x36, false), "<": (0x36, true),
        ".": (0x37, false), ">": (0x37, true),
        "/": (0x38, false), "?": (0x38, true),
        "!": (0x1E, true), "@": (0x1F, true),
        "#": (0x20, true), "$": (0x21, true),
        "%": (0x22, true), "^": (0x23, true),
        "&": (0x24, true), "*": (0x25, true),
        "(": (0x26, true), ")": (0x27, true),
    ]
}
