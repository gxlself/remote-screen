import XCTest
@testable import NativeLogic

final class KeyboardTests: XCTestCase {
    func testHidMapLettersNumbersAndUnmappableText() {
        assertMapping("a", 0x04, false)
        assertMapping("z", 0x1D, false)
        assertMapping("A", 0x04, true)
        assertMapping("Z", 0x1D, true)
        assertMapping("1", 0x1E, false)
        assertMapping("9", 0x26, false)
        assertMapping("0", 0x27, false)
        assertMapping("!", 0x1E, true)
        assertMapping("?", 0x38, true)
        assertMapping(" ", 0x2C, false)
        assertMapping("\n", 0x28, false)
        assertMapping("\t", 0x2B, false)
        XCTAssertNil(KeyboardHidMap.usage(for: "中"))
        XCTAssertNil(KeyboardHidMap.usage(for: "é"))
        XCTAssertNil(KeyboardHidMap.usage(for: "👨‍👩‍👧"))
    }

    func testHidMapAnsiPunctuationSkipsNonUSUsage() {
        let plain = Array("-=[]\\;'`,./")
        let shifted = Array("_+{}|:\"~<>?")
        let usages: [UInt16] = [0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38]
        for index in usages.indices {
            assertMapping(plain[index], usages[index], false)
            assertMapping(shifted[index], usages[index], true)
        }
        for (index, character) in "!@#$%^&*()".enumerated() {
            assertMapping(character, UInt16(index) + 0x1E, true)
        }
    }

    func testModifiersUseAfterTransitionStateAndPreserveBothSides() {
        XCTAssertEqual(KeyboardModifiers.mods(control: true, shift: true, option: true, command: true), 0x0F)
        XCTAssertEqual(KeyboardModifiers.mods(control: false, shift: false, option: false, command: false), 0)
        XCTAssertEqual(KeyboardModifiers.after(usage: 0xE3, down: true, current: 0), 0x08)
        XCTAssertEqual(KeyboardModifiers.after(usage: 0xE7, down: true, current: 0x08), 0x88)
        XCTAssertEqual(KeyboardModifiers.after(usage: 0xE3, down: false, current: 0x88), 0x80)
        for offset in UInt16(0)..<8 {
            let mask = UInt8(1) << offset
            XCTAssertEqual(KeyboardModifiers.after(usage: 0xE0 + offset, down: true, current: 0), mask)
            XCTAssertEqual(KeyboardModifiers.after(usage: 0xE0 + offset, down: false, current: 0xFF), ~mask)
        }
        XCTAssertEqual(KeyboardModifiers.after(usage: 0x06, down: false, current: 0x88), 0x88)
    }

    func testOSMapRemapsCommandToCorrespondingControlSide() {
        let windows = KeyboardOsMap.remap(usage: 0xE3, mods: 0x08, os: .windows)
        XCTAssertEqual(windows.0, 0xE0)
        XCTAssertEqual(windows.1, 0x01)
        let right = KeyboardOsMap.remap(usage: 0xE7, mods: 0x88, os: .windows)
        XCTAssertEqual(right.0, 0xE4)
        XCTAssertEqual(right.1, 0x11)
        let shortcut = KeyboardOsMap.remap(usage: 0x06, mods: 0xAA, os: .windows)
        XCTAssertEqual(shortcut.0, 0x06)
        XCTAssertEqual(shortcut.1, 0x33)
        let mac = KeyboardOsMap.remap(usage: 0xE3, mods: 0x88, os: .macos)
        XCTAssertEqual(mac.0, 0xE3)
        XCTAssertEqual(mac.1, 0x88)
    }

    func testSplitterRespectsUTF8BytesAndGraphemeBoundaries() {
        let ascii = KeyboardTextSplitter.split(String(repeating: "a", count: 5000))
        XCTAssertEqual(ascii.map { $0.utf8.count }, [4096, 904])
        let chinese = KeyboardTextSplitter.split(String(repeating: "中", count: 2000))
        XCTAssertEqual(chinese.map(\.count), [1365, 635])
        XCTAssertTrue(chinese.allSatisfy { $0.utf8.count <= 4096 })
        let family = "👨‍👩‍👧"
        let text = String(repeating: "a", count: 4090) + family + "e\u{301}"
        let emoji = KeyboardTextSplitter.split(text)
        XCTAssertEqual(emoji, [String(repeating: "a", count: 4090), family + "e\u{301}"])
        XCTAssertEqual(emoji.joined(), text)
        XCTAssertEqual(KeyboardTextSplitter.split(""), [])
    }

    func testSplitterNeverEmitsOversizeOrEmptyFrames() {
        XCTAssertEqual(KeyboardTextSplitter.split("a", maxBytes: 0), [])
        XCTAssertEqual(KeyboardTextSplitter.split("a", maxBytes: -1), [])
        XCTAssertEqual(KeyboardTextSplitter.split("a中b", maxBytes: 2), ["a", "b"])
    }

    func testTextCommitAndNewlines() {
        XCTAssertEqual(plan("你好"), [.text("你好")])
        XCTAssertEqual(plan("a\nb"), [
            .text("a"),
            .key(usage: 0x28, down: true, mods: 0),
            .key(usage: 0x28, down: false, mods: 0),
            .text("b"),
        ])
        XCTAssertEqual(plan(""), [])
        XCTAssertEqual(plan("a\tb"), [.text("a\tb")])
    }

    func testKeysCommitUsesShiftAndGroupsUnmappableCharacters() {
        XCTAssertEqual(plan("Hi", mode: .keys), [
            .key(usage: 0x0B, down: true, mods: 0x02),
            .key(usage: 0x0B, down: false, mods: 0x02),
            .key(usage: 0x0C, down: true, mods: 0),
            .key(usage: 0x0C, down: false, mods: 0),
        ])
        XCTAssertEqual(plan("中👨‍👩‍👧a文", mode: .keys), [
            .text("中👨‍👩‍👧"),
            .key(usage: 0x04, down: true, mods: 0),
            .key(usage: 0x04, down: false, mods: 0),
            .text("文"),
        ])
    }

    func testStickyConsumesExactlyOneMappableFirstCharacter() {
        XCTAssertEqual(plan("c", stickyMods: 0x08), [
            .key(usage: 0x06, down: true, mods: 0x08),
            .key(usage: 0x06, down: false, mods: 0x08),
            .stickyConsumed(0x08),
        ])
        XCTAssertEqual(plan("Cz", stickyMods: 0x80), [
            .key(usage: 0x06, down: true, mods: 0x82),
            .key(usage: 0x06, down: false, mods: 0x82),
            .stickyConsumed(0x80),
            .text("z"),
        ])
        XCTAssertEqual(plan("中", stickyMods: 0x08), [.text("中")])
        XCTAssertEqual(plan("中c", stickyMods: 0x08), [.text("中c")])
        XCTAssertEqual(plan("", stickyMods: 0x08), [])
    }

    func testPlannerSplitsTextAndKeyFallbackAtWireLimit() {
        let text = String(repeating: "中", count: 2000)
        let expected = KeyboardTextSplitter.split(text).map(KeyboardAction.text)
        XCTAssertEqual(plan(text), expected)
        XCTAssertEqual(plan(text, mode: .keys), expected)
    }

    // MARK: Local key repeat

    func testRepeatStartsAfterDelayThenEveryInterval() {
        var repeater = KeyboardRepeat()
        repeater.keyDown(0x50, at: 10) // Left Arrow
        XCTAssertEqual(repeater.usage, 0x50)
        XCTAssertEqual(repeater.nextFire, 10.5)
        XCTAssertNil(repeater.fire(at: 10.499))
        XCTAssertEqual(repeater.fire(at: 10.5), 0x50)
        XCTAssertEqual(repeater.nextFire!, 10.533, accuracy: 1e-9)
        XCTAssertNil(repeater.fire(at: 10.52))
        XCTAssertEqual(repeater.fire(at: 10.5331), 0x50)
        XCTAssertEqual(repeater.nextFire!, 10.566, accuracy: 1e-9)
        XCTAssertEqual(KeyboardRepeat.delay, 0.5)
        XCTAssertEqual(KeyboardRepeat.interval, 0.033)
    }

    func testLateTimerSendsOneRepeatAndKeepsCadenceFromNow() {
        var repeater = KeyboardRepeat()
        repeater.keyDown(0x2A, at: 0) // Backspace
        XCTAssertEqual(repeater.fire(at: 2), 0x2A)
        XCTAssertEqual(repeater.nextFire!, 2.033, accuracy: 1e-9)
        XCTAssertNil(repeater.fire(at: 2.01))
    }

    func testModifiersAndLockKeysNeverRepeat() {
        for usage in [UInt16(0xE0), 0xE1, 0xE3, 0xE7, 0x39, 0x47, 0x53] {
            XCTAssertFalse(KeyboardRepeat.repeats(usage: usage), "\(usage)")
            var repeater = KeyboardRepeat()
            repeater.keyDown(usage, at: 0)
            XCTAssertNil(repeater.usage)
            XCTAssertNil(repeater.fire(at: 1))
        }
        for usage in [UInt16(0x4F), 0x4A, 0x4D, 0x4B, 0x4E, 0x4C, 0x2A, 0x29, 0x04] {
            XCTAssertTrue(KeyboardRepeat.repeats(usage: usage), "\(usage)")
        }
    }

    func testModifierDownKeepsRepeatAndNewKeyTakesOver() {
        var repeater = KeyboardRepeat()
        repeater.keyDown(0x4F, at: 0)
        repeater.keyDown(0xE1, at: 0.2) // Shift while holding Right Arrow
        XCTAssertEqual(repeater.usage, 0x4F)
        XCTAssertEqual(repeater.nextFire, 0.5)
        repeater.keyDown(0x51, at: 0.3) // Down Arrow
        XCTAssertEqual(repeater.usage, 0x51)
        XCTAssertEqual(repeater.nextFire!, 0.8, accuracy: 1e-9)
        // Releasing the earlier key leaves the newer one repeating.
        XCTAssertFalse(repeater.keyUp(0x4F))
        XCTAssertEqual(repeater.fire(at: 0.81), 0x51)
    }

    func testRepeatStopsForEveryReason() {
        let reasons: [KeyboardRepeat.StopReason] = [.keyUp, .focusLost, .background, .keyboardDisconnected]
        for reason in reasons {
            var repeater = KeyboardRepeat()
            repeater.keyDown(0x52, at: 0)
            XCTAssertTrue(repeater.stop(reason))
            XCTAssertNil(repeater.usage)
            XCTAssertNil(repeater.nextFire)
            XCTAssertNil(repeater.fire(at: 5))
            XCTAssertFalse(repeater.stop(reason))
        }
        var repeater = KeyboardRepeat()
        repeater.keyDown(0x52, at: 0)
        XCTAssertTrue(repeater.keyUp(0x52))
        XCTAssertNil(repeater.fire(at: 1))
        XCTAssertFalse(repeater.keyUp(0x52))
    }

    // MARK: Focus

    func testHardwareCaptureWaitsForOtherFieldsButTheBarDoesNot() {
        func capture(wanted: Bool = false, hardware: Bool = true, others: Bool = false,
                     configured: Bool = true, active: Bool = true, inWindow: Bool = true) -> Bool {
            KeyboardFocusPolicy.shouldCapture(configured: configured, appActive: active, inWindow: inWindow,
                                              wanted: wanted, hardware: hardware, othersEditing: others)
        }
        XCTAssertTrue(capture())
        XCTAssertFalse(capture(others: true))
        XCTAssertTrue(capture(wanted: true, others: true))
        XCTAssertTrue(capture(wanted: true, hardware: false))
        XCTAssertFalse(capture(hardware: false))
        XCTAssertFalse(capture(configured: false))
        XCTAssertFalse(capture(active: false))
        XCTAssertFalse(capture(wanted: true, inWindow: false))
    }

    // MARK: Command-period and paste

    func testCommandPeriodSendsEscapeAroundHeldCommand() {
        let held = KeyboardPassthrough.keyEvents(for: KeyboardPassthrough.commandPeriod,
                                                 heldModifiers: 0x08, commandPeriodAsEscape: true)
        XCTAssertEqual(held, [
            .init(usage: 0xE3, down: false, mods: 0),
            .init(usage: 0x29, down: true, mods: 0),
            .init(usage: 0x29, down: false, mods: 0),
            .init(usage: 0xE3, down: true, mods: 0x08),
        ])
        let both = KeyboardPassthrough.escapeEvents(heldModifiers: 0x89)
        XCTAssertEqual(both, [
            .init(usage: 0xE3, down: false, mods: 0x81),
            .init(usage: 0xE7, down: false, mods: 0x01),
            .init(usage: 0x29, down: true, mods: 0x01),
            .init(usage: 0x29, down: false, mods: 0x01),
            .init(usage: 0xE7, down: true, mods: 0x81),
            .init(usage: 0xE3, down: true, mods: 0x89),
        ])
        // Without a held Command (UIKeyCommand only), just Escape.
        XCTAssertEqual(KeyboardPassthrough.escapeEvents(heldModifiers: 0), [
            .init(usage: 0x29, down: true, mods: 0),
            .init(usage: 0x29, down: false, mods: 0),
        ])
    }

    func testCommandPeriodStaysItselfOffIPadAndWithShift() {
        let plain = KeyboardPassthrough.keyEvents(for: KeyboardPassthrough.commandPeriod,
                                                  heldModifiers: 0x08, commandPeriodAsEscape: false)
        XCTAssertEqual(plain, KeyboardPassthrough.keyEvents(for: KeyboardPassthrough.commandPeriod,
                                                            heldModifiers: 0x08))
        XCTAssertTrue(plain.contains(.init(usage: 0x37, down: true, mods: 0x08)))
        let shifted = KeyboardPassthrough.Combo(input: ".", usage: 0x37, mods: 0x0A)
        XCTAssertTrue(KeyboardPassthrough.commandCombos.contains(shifted))
        XCTAssertEqual(
            KeyboardPassthrough.keyEvents(for: shifted, heldModifiers: 0x0A, commandPeriodAsEscape: true),
            KeyboardPassthrough.keyEvents(for: shifted, heldModifiers: 0x0A))
    }

    func testPasteAndCommandPeriodAreKnownCommandCombos() {
        XCTAssertEqual(KeyboardPassthrough.commandCombo(usage: 0x19, mods: 0x08), KeyboardPassthrough.commandV)
        XCTAssertEqual(KeyboardPassthrough.commandCombo(usage: 0x37, mods: 0x80), KeyboardPassthrough.commandPeriod)
    }

    private func assertMapping(
        _ character: Character, _ usage: UInt16, _ shift: Bool,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        let mapped = KeyboardHidMap.usage(for: character)
        XCTAssertEqual(mapped?.usage, usage, file: file, line: line)
        XCTAssertEqual(mapped?.shift, shift, file: file, line: line)
    }

    private func plan(
        _ committed: String, mode: KeyboardInputMode = .text, stickyMods: UInt8 = 0
    ) -> [KeyboardAction] {
        KeyboardCommitPlanner.plan(committed: committed, mode: mode, stickyMods: stickyMods)
    }
}
