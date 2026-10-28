import Foundation

public enum TerminalKeySequences {
    public static func bytes(name: String, appCursor: Bool, mods: Int32) -> [UInt8] {
        let modifierBits = UInt8(truncatingIfNeeded: mods)
        let modifier = xtermModifier(modifierBits)
        switch name {
        case "esc":
            return TerminalModifierApply.apply(bytes: [0x1b], mods: mods).bytes
        case "tab":
            return TerminalModifierApply.apply(bytes: [0x09], mods: mods).bytes
        case "up":
            return csi(final: "A", appCursor: appCursor, modifier: modifier)
        case "down":
            return csi(final: "B", appCursor: appCursor, modifier: modifier)
        case "right":
            return csi(final: "C", appCursor: appCursor, modifier: modifier)
        case "left":
            return csi(final: "D", appCursor: appCursor, modifier: modifier)
        case "home":
            return csi(final: "H", appCursor: appCursor, modifier: modifier)
        case "end":
            return csi(final: "F", appCursor: appCursor, modifier: modifier)
        case "pageUp":
            return tilde(prefix: "5", modifier: modifier)
        case "pageDown":
            return tilde(prefix: "6", modifier: modifier)
        default:
            guard name.count == 1 else { return [] }
            return TerminalModifierApply.apply(bytes: Array(name.utf8), mods: mods).bytes
        }
    }

    private static func xtermModifier(_ mods: UInt8) -> Int {
        let alt = mods & 0x02 != 0 ? 2 : 0
        let ctrl = mods & 0x01 != 0 ? 4 : 0
        return 1 + alt + ctrl
    }

    private static func csi(final: String, appCursor: Bool, modifier: Int) -> [UInt8] {
        if modifier == 1 {
            return Array(("\u{1b}" + (appCursor ? "O" : "[") + final).utf8)
        }
        return Array(("\u{1b}[1;\(modifier)\(final)").utf8)
    }

    private static func tilde(prefix: String, modifier: Int) -> [UInt8] {
        if modifier == 1 { return Array(("\u{1b}[\(prefix)~").utf8) }
        return Array(("\u{1b}[\(prefix);\(modifier)~").utf8)
    }
}
