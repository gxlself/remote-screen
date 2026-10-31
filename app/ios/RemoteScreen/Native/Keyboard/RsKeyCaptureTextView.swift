import UIKit

/// Lets UIKit own printable text/IME input, intercepting only keys which must
/// retain their HID identity. No key or text content is logged here.
final class RsKeyCaptureTextView: UITextView {
    var captureMode: KeyboardInputMode = .text
    /// A hardware keyboard captured while the on-screen keyboard is closed.
    /// UIKit shows keyboard UI only for an editable text view; iPadOS would
    /// otherwise float its shortcut bar over the bottom of the picture. Every
    /// key goes out raw, so the computer's own IME composes text.
    var hardwareOnly = false {
        didSet { if hardwareOnly != oldValue { isEditable = !hardwareOnly } }
    }
    var onRawKey: ((UInt16, Bool, UInt8) -> Void)?
    /// Cmd+V asks JS to push the clipboard first; JS then sends the paste.
    var syncPaste = false
    /// Held modifiers at a synced Cmd+V, before the OS remap.
    var onPaste: ((UInt8) -> Void)?
    /// Magic Keyboard has no Escape key; Cmd+. stands in for it on iPad.
    var commandPeriodAsEscape = UIDevice.current.userInterfaceIdiom == .pad
    /// ⌃⌥⌘ session shortcuts; handled on this device, never sent to the computer.
    var onShortcut: ((SessionShortcut) -> Void)?

    private enum Route {
        case raw
        case modifier
        case text
        case command
        /// A session shortcut: down and up both stay on this device.
        case local
    }

    private var pressed: [UInt16: Route] = [:]
    private var modifiers: UInt8 = 0
    private var reportedCommandPath = false
    private var keyRepeat = KeyboardRepeat()
    private var repeatTimer: Timer?

    private lazy var passthroughCommands: [UIKeyCommand] = KeyboardPassthrough.commandCombos.map { combo in
        var flags: UIKeyModifierFlags = [.command]
        if combo.mods & 0x02 != 0 { flags.insert(.shift) }
        let command = UIKeyCommand(input: combo.input, modifierFlags: flags,
                                   action: #selector(sendCommandCombo(_:)))
        command.wantsPriorityOverSystemBehavior = true
        command.allowsAutomaticLocalization = false
        command.allowsAutomaticMirroring = false
        return command
    }

    override var keyCommands: [UIKeyCommand]? { passthroughCommands }
    /// while this hidden view is first responder, iPadOS would
    /// read three-finger swipes as undo / redo and a three-finger pinch as
    /// copy / paste. Those fingers switch computers instead.
    override var editingInteractionConfiguration: UIEditingInteractionConfiguration { .none }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        // This invisible text view owns no local document. Disable all edit
        // actions (copy/paste/cut/selectAll/undo/redo/find, including new OS
        // variants) while leaving our remote Command handler available.
        action == #selector(sendCommandCombo(_:)) && isFirstResponder
    }

    @objc private func sendCommandCombo(_ command: UIKeyCommand) {
        guard let combo = KeyboardPassthrough.commandCombos.first(where: {
            $0.input == command.input && ($0.mods & 0x02 != 0) == command.modifierFlags.contains(.shift)
        }) else { return }
        if !reportedCommandPath {
            reportedCommandPath = true
            RsLog.marker("KEYCAP", ["route": "keyCommands"])
        }
        if syncPaste, combo == KeyboardPassthrough.commandV, let onPaste {
            onPaste(modifiers)
            return
        }
        for event in KeyboardPassthrough.keyEvents(for: combo, heldModifiers: modifiers,
                                                   commandPeriodAsEscape: commandPeriodAsEscape) {
            onRawKey?(event.usage, event.down, event.mods)
        }
    }

    override func resignFirstResponder() -> Bool {
        stopRepeat(.focusLost)
        return super.resignFirstResponder()
    }

    /// Ends a local repeat; the key-up itself goes out with the key's release.
    func stopRepeat(_ reason: KeyboardRepeat.StopReason) {
        if keyRepeat.stop(reason) { scheduleRepeat() }
    }

    private func scheduleRepeat() {
        repeatTimer?.invalidate()
        repeatTimer = nil
        guard let due = keyRepeat.nextFire else { return }
        let timer = Timer(timeInterval: max(0, due - CACurrentMediaTime()), repeats: false) { [weak self] _ in
            self?.repeatTick()
        }
        // Keeps firing while a scroll view tracks a touch.
        RunLoop.main.add(timer, forMode: .common)
        repeatTimer = timer
    }

    private func repeatTick() {
        repeatTimer = nil
        if let usage = keyRepeat.fire(at: CACurrentMediaTime()) {
            onRawKey?(usage, true, modifiers)
        }
        scheduleRepeat()
    }

    override func deleteBackward() {
        if markedTextRange == nil, text.isEmpty {
            onRawKey?(0x2A, true, modifiers)
            onRawKey?(0x2A, false, modifiers)
        } else {
            super.deleteBackward()
        }
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var forwarded: Set<UIPress> = []
        // Modifier transitions precede regular keyDowns, including when UIKit
        // delivers both sides in the same batch (the input Set has no order).
        for press in ordered(presses, down: true) {
            guard let key = press.key, let usage = UInt16(exactly: key.keyCode.rawValue) else {
                forwarded.insert(press)
                continue
            }
            updateModifiers(key: key, usage: usage, down: true)
            // There is no repeat timer, and repeated began events produce only
            // the original down. End/cancel supplies its matching up.
            guard pressed[usage] == nil else { continue }
            let route = route(for: usage, flags: key.modifierFlags)
            pressed[usage] = route
            switch route {
            case .raw:
                onRawKey?(usage, true, modifiers)
                keyRepeat.keyDown(usage, at: CACurrentMediaTime())
                scheduleRepeat()
            case .modifier:
                onRawKey?(usage, true, modifiers)
                forwarded.insert(press)
            case .text:
                forwarded.insert(press)
            case .command:
                // Let UIKit resolve our priority UIKeyCommand. Sending this
                // same key through onRawKey would duplicate the shortcut.
                forwarded.insert(press)
            case .local:
                if let shortcut = KeyboardPassthrough.sessionShortcut(usage: usage, mods: modifiers) {
                    onShortcut?(shortcut)
                }
            }
        }
        if !forwarded.isEmpty { super.pressesBegan(forwarded, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let forwarded = finish(presses)
        if !forwarded.isEmpty { super.pressesEnded(forwarded, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let forwarded = finish(presses)
        if !forwarded.isEmpty { super.pressesCancelled(forwarded, with: event) }
    }

    /// Avoid leaving remote keys held when the responder/OS mapping is lost.
    func releasePressedKeys() {
        stopRepeat(.focusLost)
        let usages = pressed.keys.sorted { left, right in
            let leftModifier = Self.isModifier(left)
            let rightModifier = Self.isModifier(right)
            return leftModifier == rightModifier ? left < right : !leftModifier
        }
        for usage in usages {
            guard let route = pressed[usage] else { continue }
            modifiers = KeyboardModifiers.after(usage: usage, down: false, current: modifiers)
            if route == .raw || route == .modifier { onRawKey?(usage, false, modifiers) }
        }
        // UIKey flags can reveal modifiers held before focus was acquired.
        // A Command action restores those too, so release them even when no
        // matching modifier pressesBegan was delivered to this responder.
        for usage in UInt16(0xE0)...0xE7 where modifiers & (UInt8(1) << (usage - 0xE0)) != 0 {
            modifiers = KeyboardModifiers.after(usage: usage, down: false, current: modifiers)
            onRawKey?(usage, false, modifiers)
        }
        pressed.removeAll()
        modifiers = 0
    }

    private func finish(_ presses: Set<UIPress>) -> Set<UIPress> {
        var forwarded: Set<UIPress> = []
        for press in ordered(presses, down: false) {
            guard let key = press.key, let usage = UInt16(exactly: key.keyCode.rawValue) else {
                forwarded.insert(press)
                continue
            }
            updateModifiers(key: key, usage: usage, down: false)
            // Use the down's route even if mode or modifiers changed while held.
            guard let route = pressed.removeValue(forKey: usage) ?? (Self.isModifier(usage) ? .modifier : nil) else {
                forwarded.insert(press)
                continue
            }
            switch route {
            case .raw:
                if keyRepeat.keyUp(usage) { scheduleRepeat() }
                onRawKey?(usage, false, modifiers)
            case .modifier:
                onRawKey?(usage, false, modifiers)
                forwarded.insert(press)
            case .text:
                forwarded.insert(press)
            case .command:
                forwarded.insert(press)
            case .local:
                break
            }
        }
        return forwarded
    }

    private func route(for usage: UInt16, flags: UIKeyModifierFlags) -> Route {
        if Self.isModifier(usage) { return .modifier }
        if onShortcut != nil, KeyboardPassthrough.sessionShortcut(usage: usage, mods: modifiers) != nil {
            return .local
        }
        // UIKeyModifierFlags has no Fn/Globe bit, so globe combos cannot be told apart here.
        if KeyboardPassthrough.commandCombo(usage: usage, mods: modifiers) != nil {
            return .command
        }
        if captureMode == .keys || hardwareOnly || modifiers & 0xDD != 0 || Self.isNonPrintable(usage) {
            return .raw
        }
        if usage == 0x2A, text.isEmpty, markedTextRange == nil { return .raw }
        return .text
    }

    private func updateModifiers(key: UIKey, usage: UInt16, down: Bool) {
        let flags = key.modifierFlags
        let aggregate = KeyboardModifiers.mods(
            control: flags.contains(.control),
            shift: flags.contains(.shift),
            option: flags.contains(.alternate),
            command: flags.contains(.command)
        )
        for index in UInt16(0)..<4 {
            let left = UInt8(1) << index
            let both = left | (left << 4)
            // UIKey flags collapse left/right. Never infer a left modifier for
            // the modifier currently transitioning (e.g. first right Command).
            if usage == 0xE0 + index || usage == 0xE4 + index { continue }
            if aggregate & left == 0 {
                modifiers &= ~both
            } else if modifiers & both == 0 {
                // A modifier may have been held before this view became the
                // responder. Its side is unavailable; fall back to the left.
                modifiers |= left
            }
        }
        modifiers = KeyboardModifiers.after(usage: usage, down: down, current: modifiers)
    }

    private func ordered(_ presses: Set<UIPress>, down: Bool) -> [UIPress] {
        presses.sorted { left, right in
            let leftUsage = left.key?.keyCode.rawValue ?? 0
            let rightUsage = right.key?.keyCode.rawValue ?? 0
            let leftModifier = (0xE0...0xE7).contains(leftUsage)
            let rightModifier = (0xE0...0xE7).contains(rightUsage)
            if leftModifier != rightModifier { return down ? leftModifier : !leftModifier }
            return leftUsage < rightUsage
        }
    }

    private static func isModifier(_ usage: UInt16) -> Bool {
        (0xE0...0xE7).contains(usage)
    }

    private static func isNonPrintable(_ usage: UInt16) -> Bool {
        switch usage {
        case 0x29, 0x2B, // Escape, Tab
             0x39...0x53, // Caps Lock, F1–F12, navigation, Insert, Forward Delete, Num Lock
             0x58, // keypad Enter
             0x68...0x73: // F13–F24
            return true
        default:
            return false
        }
    }
}
