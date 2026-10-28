import XCTest
@testable import NativeLogic

final class SessionShortcutTests: XCTestCase {
    private let leftControlOptionCommand: UInt8 = 0x01 | 0x04 | 0x08
    private let rightControlOptionCommand: UInt8 = 0x10 | 0x40 | 0x80

    func testArrowsGoToTheNextAndPreviousSession() {
        XCTAssertEqual(KeyboardPassthrough.sessionShortcut(usage: 0x4F, mods: leftControlOptionCommand), .next)
        XCTAssertEqual(KeyboardPassthrough.sessionShortcut(usage: 0x50, mods: leftControlOptionCommand), .previous)
    }

    func testDigitsOneToNinePickASession() {
        for digit in 1...9 {
            let usage = UInt16(0x1E + digit - 1)
            let shortcut = KeyboardPassthrough.sessionShortcut(usage: usage, mods: leftControlOptionCommand)
            XCTAssertEqual(shortcut, .index(digit))
            XCTAssertEqual(shortcut?.action, "index")
            XCTAssertEqual(shortcut?.index, digit)
        }
        // 0 is not a session number.
        XCTAssertNil(KeyboardPassthrough.sessionShortcut(usage: 0x27, mods: leftControlOptionCommand))
    }

    func testSOpensTheSwitcher() {
        let shortcut = KeyboardPassthrough.sessionShortcut(usage: 0x16, mods: leftControlOptionCommand)
        XCTAssertEqual(shortcut, .switcher)
        XCTAssertEqual(shortcut?.action, "switcher")
        XCTAssertEqual(shortcut?.index, 0)
        XCTAssertEqual(SessionShortcut.next.action, "next")
        XCTAssertEqual(SessionShortcut.previous.action, "previous")
    }

    func testEitherSideOfEachModifierCounts() {
        XCTAssertEqual(KeyboardPassthrough.sessionShortcut(usage: 0x4F, mods: rightControlOptionCommand), .next)
        // Right Control, left Option, right Command.
        XCTAssertEqual(KeyboardPassthrough.sessionShortcut(usage: 0x50, mods: 0x10 | 0x04 | 0x80), .previous)
        XCTAssertEqual(KeyboardPassthrough.sessionShortcut(usage: 0x16, mods: 0xFF & ~0x22), .switcher)
    }

    func testOtherModifierSetsReachTheComputer() {
        let others: [UInt8] = [0x00, 0x08, 0x09, 0x0C, 0x05, 0x0F, 0x0D | 0x02, 0x0D | 0x20]
        for mods in others {
            XCTAssertNil(KeyboardPassthrough.sessionShortcut(usage: 0x4F, mods: mods), "mods \(mods)")
            XCTAssertNil(KeyboardPassthrough.sessionShortcut(usage: 0x1E, mods: mods), "mods \(mods)")
            XCTAssertNil(KeyboardPassthrough.sessionShortcut(usage: 0x16, mods: mods), "mods \(mods)")
        }
    }

    func testOtherKeysReachTheComputer() {
        for usage: UInt16 in [0x04, 0x15, 0x17, 0x27, 0x28, 0x29, 0x51, 0x52, 0xE3] {
            XCTAssertNil(KeyboardPassthrough.sessionShortcut(usage: usage, mods: leftControlOptionCommand))
        }
    }

    func testShortcutsAreNotRemoteCommandCombos() {
        // Not a Command / Shift-Command combo, so the key view does not hand
        // them to a UIKeyCommand that would send them to the computer.
        for usage: UInt16 in [0x4F, 0x50, 0x1E, 0x26, 0x16] {
            XCTAssertNil(KeyboardPassthrough.commandCombo(usage: usage, mods: leftControlOptionCommand))
        }
    }
}
