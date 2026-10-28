import Foundation

public enum TerminalModifierApply {
    public static func apply(bytes: [UInt8], mods: Int32) -> (bytes: [UInt8], consumed: Bool) {
        let modifiers = UInt8(truncatingIfNeeded: mods) & 0x03
        guard modifiers != 0, !bytes.isEmpty,
              let character = String(bytes: bytes, encoding: .utf8), character.count == 1 else {
            return (bytes, false)
        }

        var value = bytes
        var changed = false
        if modifiers & 0x01 != 0, bytes.count == 1, let byte = bytes.first,
           let control = controlByte(byte) {
            value = [control]
            changed = true
        }
        if modifiers & 0x02 != 0 {
            return ([0x1b] + value, true)
        }
        return (value, changed)
    }

    private static func controlByte(_ byte: UInt8) -> UInt8? {
        switch byte {
        case 0x61...0x7a, 0x41...0x5a:
            return byte & 0x1f
        case 0x40, 0x20:
            return 0x00
        case 0x5b:
            return 0x1b
        case 0x5c:
            return 0x1c
        case 0x5d:
            return 0x1d
        case 0x5e:
            return 0x1e
        case 0x5f:
            return 0x1f
        case 0x3f:
            return 0x7f
        default:
            return nil
        }
    }
}
