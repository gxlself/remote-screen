enum KeyboardModifiers {
    static func mods(control: Bool, shift: Bool, option: Bool, command: Bool) -> UInt8 {
        (control ? 0x01 : 0) | (shift ? 0x02 : 0)
            | (option ? 0x04 : 0) | (command ? 0x08 : 0)
    }

    /// The wire mask describes the state *after* this transition, including
    /// the modifier key itself. The upper four bits preserve the right side.
    static func after(usage: UInt16, down: Bool, current: UInt8) -> UInt8 {
        guard (0xE0...0xE7).contains(usage) else { return current }
        let bit = UInt8(1) << (usage - 0xE0)
        return down ? current | bit : current & ~bit
    }
}
