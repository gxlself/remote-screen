enum KeyboardOS: String {
    case macos
    case windows
}

enum KeyboardOsMap {
    static func remap(usage: UInt16, mods: UInt8, os: KeyboardOS) -> (UInt16, UInt8) {
        guard os == .windows else { return (usage, mods) }
        let mappedUsage: UInt16
        switch usage {
        case 0xE3: mappedUsage = 0xE0
        case 0xE7: mappedUsage = 0xE4
        default: mappedUsage = usage
        }
        let mappedMods = (mods & ~0x88)
            | (mods & 0x08 == 0 ? 0 : 0x01)
            | (mods & 0x80 == 0 ? 0 : 0x10)
        return (mappedUsage, mappedMods)
    }
}
