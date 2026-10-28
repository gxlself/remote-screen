import Foundation
import XCTest
@testable import NativeLogic

final class TerminalTests: XCTestCase {
    func testModifierApply() {
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x63], mods: 1).bytes, [0x03])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x43], mods: 1).bytes, [0x03])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x5b], mods: 1).bytes, [0x1b])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x3f], mods: 1).bytes, [0x7f])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x62], mods: 2).bytes, [0x1b, 0x62])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x63], mods: 3).bytes, [0x1b, 0x03])
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x63], mods: 0).bytes, [0x63])
        XCTAssertFalse(TerminalModifierApply.apply(bytes: [0x63], mods: 0).consumed)
        XCTAssertEqual(TerminalModifierApply.apply(bytes: [0x1b, 0x5b, 0x41], mods: 3).bytes,
                       [0x1b, 0x5b, 0x41])
        XCTAssertFalse(TerminalModifierApply.apply(bytes: [0x1b, 0x5b, 0x41], mods: 3).consumed)

        let controlCases: [(String, UInt8)] = [
            ("@", 0x00), (" ", 0x00), ("\\", 0x1c), ("]", 0x1d), ("^", 0x1e), ("_", 0x1f),
        ]
        for (key, expected) in controlCases {
            let applied = TerminalModifierApply.apply(bytes: Array(key.utf8), mods: 1)
            XCTAssertEqual(applied.bytes, [expected], key)
            XCTAssertTrue(applied.consumed, key)
        }
        XCTAssertFalse(TerminalModifierApply.apply(bytes: [0x31], mods: 1).consumed)
        XCTAssertEqual(TerminalModifierApply.apply(bytes: Array("é".utf8), mods: 2).bytes,
                       [0x1b] + Array("é".utf8))
        XCTAssertFalse(TerminalModifierApply.apply(bytes: Array("ab".utf8), mods: 2).consumed)
        XCTAssertFalse(TerminalModifierApply.apply(bytes: [0xff], mods: 2).consumed)
    }

    func testKeySequences() {
        XCTAssertEqual(TerminalKeySequences.bytes(name: "up", appCursor: false, mods: 0), [0x1b, 0x5b, 0x41])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "up", appCursor: true, mods: 0), [0x1b, 0x4f, 0x41])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "right", appCursor: false, mods: 1),
                       [0x1b, 0x5b, 0x31, 0x3b, 0x35, 0x43])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "left", appCursor: false, mods: 2),
                       [0x1b, 0x5b, 0x31, 0x3b, 0x33, 0x44])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "esc", appCursor: false, mods: 0), [0x1b])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "esc", appCursor: false, mods: 2), [0x1b, 0x1b])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "tab", appCursor: false, mods: 0), [0x09])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "|", appCursor: false, mods: 0), [0x7c])
        XCTAssertEqual(TerminalKeySequences.bytes(name: "home", appCursor: false, mods: 0),
                       Array("\u{1b}[H".utf8))
        XCTAssertEqual(TerminalKeySequences.bytes(name: "end", appCursor: true, mods: 0),
                       Array("\u{1b}OF".utf8))
        XCTAssertEqual(TerminalKeySequences.bytes(name: "down", appCursor: true, mods: 3),
                       Array("\u{1b}[1;7B".utf8))
        XCTAssertEqual(TerminalKeySequences.bytes(name: "pageUp", appCursor: false, mods: 0),
                       Array("\u{1b}[5~".utf8))
        XCTAssertEqual(TerminalKeySequences.bytes(name: "pageDown", appCursor: false, mods: 1),
                       Array("\u{1b}[6;5~".utf8))
        XCTAssertEqual(TerminalKeySequences.bytes(name: "é", appCursor: false, mods: 2),
                       [0x1b] + Array("é".utf8))
        XCTAssertTrue(TerminalKeySequences.bytes(name: "unknown", appCursor: false, mods: 3).isEmpty)
    }

    func testPasteEncoder() {
        XCTAssertEqual(TerminalPasteEncoder.encode(text: "a\nb", bracketed: false), [0x61, 0x0d, 0x62])
        XCTAssertEqual(TerminalPasteEncoder.encode(text: "a\nb", bracketed: true),
                       Array("\u{1b}[200~a\rb\u{1b}[201~".utf8))
        XCTAssertEqual(TerminalPasteEncoder.encode(text: "x\u{1b}[201~y", bracketed: false),
                       Array("x[201~y".utf8))
        XCTAssertEqual(TerminalPasteEncoder.encode(text: "a\r\nb\rc\nd", bracketed: false),
                       Array("a\rb\rc\rd".utf8))
        XCTAssertEqual(TerminalPasteEncoder.encode(text: "\u{1b}[201~\u{1b}", bracketed: true),
                       Array("\u{1b}[200~[201~\u{1b}[201~".utf8))
    }

    func testOutputCoalescer() {
        let coalescer = TerminalOutputCoalescer()
        coalescer.append(Data(repeating: 0x41, count: 100 * 1024))
        coalescer.append(Data(repeating: 0x42, count: 100 * 1024))
        coalescer.append(Data(repeating: 0x43, count: 100 * 1024))
        XCTAssertEqual(coalescer.pendingBytes, 300 * 1024)
        XCTAssertEqual(coalescer.take(max: 256 * 1024),
                       Data(repeating: 0x41, count: 100 * 1024)
                        + Data(repeating: 0x42, count: 100 * 1024)
                        + Data(repeating: 0x43, count: 56 * 1024))
        XCTAssertEqual(coalescer.pendingBytes, 44 * 1024)
        XCTAssertEqual(coalescer.take(max: 256 * 1024), Data(repeating: 0x43, count: 44 * 1024))
        XCTAssertNil(coalescer.take(max: 256 * 1024))
        XCTAssertEqual(coalescer.pendingBytes, 0)

        let sliced = Data([0, 1, 2, 3, 4]).dropFirst(2)
        coalescer.append(sliced)
        XCTAssertNil(coalescer.take(max: 0))
        XCTAssertEqual(coalescer.take(max: 2), Data([2, 3]))
        XCTAssertEqual(coalescer.take(max: 2), Data([4]))
    }

    func testOutputCoalescerConcurrentProducers() {
        let concurrent = TerminalOutputCoalescer()
        DispatchQueue.concurrentPerform(iterations: 2) { producer in
            for index in 0..<1000 {
                concurrent.append([UInt8(producer), UInt8(index >> 8), UInt8(index & 0xff)])
            }
        }
        XCTAssertEqual(concurrent.pendingBytes, 6000)
        let bytes = Array(concurrent.take(max: 6000) ?? Data())
        var next = [0, 0]
        for offset in stride(from: 0, to: bytes.count, by: 3) {
            let producer = Int(bytes[offset])
            guard producer < next.count else { XCTFail("Invalid producer byte"); return }
            let index = Int(bytes[offset + 1]) << 8 | Int(bytes[offset + 2])
            XCTAssertEqual(index, next[producer])
            next[producer] += 1
        }
        XCTAssertEqual(next, [1000, 1000])
        XCTAssertNil(concurrent.take(max: 1))
    }

    func testFontSizeClamp() {
        XCTAssertEqual(TerminalFontSize.clamp(4), 9)
        XCTAssertEqual(TerminalFontSize.clamp(40), 24)
        XCTAssertEqual(TerminalFontSize.clamp(13.5), 13.5)
        XCTAssertEqual(TerminalFontSize.clamp(.nan), 13)
    }

    func testNightPalette() {
        let theme = TerminalTheme.night
        XCTAssertEqual(theme.ansi.count, 16)
        XCTAssertEqual(theme.background, TerminalRGB(red: 7, green: 9, blue: 11))
        XCTAssertEqual(theme.foreground, TerminalRGB(red: 213, green: 220, blue: 224))
        XCTAssertEqual(theme.cursor, TerminalRGB(red: 236, green: 240, blue: 242))
        XCTAssertEqual(theme.ansi[2], TerminalRGB(red: 93, green: 211, blue: 154))
        XCTAssertEqual(theme.ansi[8], TerminalRGB(red: 134, green: 146, blue: 154))
    }
}
