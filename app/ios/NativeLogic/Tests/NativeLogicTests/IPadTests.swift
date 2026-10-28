import CoreGraphics
import XCTest
@testable import NativeLogic

final class IPadTests: XCTestCase {
    func testProMotionRange() {
        XCTAssertEqual(DisplayFrameRatePolicy.range(maxFps: 120, lowPower: false, streaming: true),
                       .init(minimum: 80, maximum: 120, preferred: 120))
    }

    func testLowPowerRange() {
        XCTAssertEqual(DisplayFrameRatePolicy.range(maxFps: 120, lowPower: true, streaming: true),
                       .init(minimum: 30, maximum: 60, preferred: 60))
    }

    func testOrdinaryDisplaysAndStoppedStreamsDoNotRequestADisplayLink() {
        XCTAssertNil(DisplayFrameRatePolicy.range(maxFps: 60, lowPower: false, streaming: true))
        XCTAssertNil(DisplayFrameRatePolicy.range(maxFps: 60, lowPower: true, streaming: true))
        XCTAssertNil(DisplayFrameRatePolicy.range(maxFps: 120, lowPower: false, streaming: false))
        XCTAssertNil(DisplayFrameRatePolicy.range(maxFps: 120, lowPower: true, streaming: false))
    }

    func testRangeNeverExceedsTheScreenOr120Hz() {
        XCTAssertEqual(DisplayFrameRatePolicy.range(maxFps: 90, lowPower: false, streaming: true),
                       .init(minimum: 80, maximum: 90, preferred: 90))
        XCTAssertEqual(DisplayFrameRatePolicy.range(maxFps: 144, lowPower: false, streaming: true),
                       .init(minimum: 80, maximum: 120, preferred: 120))
    }

    func testCommandCombosIncludeCopyAndShiftCommandZ() {
        XCTAssertNotNil(KeyboardPassthrough.commandCombo(usage: 0x06, mods: 0x08))
        XCTAssertFalse(KeyboardPassthrough.isReserved(usage: 0x06, mods: 0x08))
        XCTAssertNotNil(KeyboardPassthrough.commandCombo(usage: 0x1D, mods: 0x0A))
        XCTAssertEqual(KeyboardPassthrough.commandCombos.count, 94)
        XCTAssertEqual(Set(KeyboardPassthrough.commandCombos).count, 94)
        XCTAssertNil(KeyboardPassthrough.commandCombo(usage: 0x06, mods: 0x09))
    }

    func testSystemReservedCombosAndRightModifiers() {
        let combinations: [(UInt16, UInt8)] = [
            (0x2B, 0x08), (0x2B, 0x0A), (0x2C, 0x08), (0x0B, 0x08),
            (0x20, 0x0A), (0x21, 0x0A), (0x22, 0x0A), (0x07, 0x0C),
            (0x2B, 0x80), (0x2B, 0xA0),
        ]
        for (usage, mods) in combinations {
            XCTAssertTrue(KeyboardPassthrough.isReserved(usage: usage, mods: mods))
        }
        XCTAssertTrue(KeyboardPassthrough.systemReserved.contains(.globeCombinations))
        XCTAssertTrue(KeyboardPassthrough.isReserved(usage: 0x4F, mods: 0, globe: true))
        XCTAssertFalse(KeyboardPassthrough.isReserved(usage: 0x06, mods: 0x80))
        XCTAssertEqual(KeyboardPassthrough.commandCombo(usage: 0x1D, mods: 0xA0)?.mods, 0x0A)
    }

    func testCommandActionSendsCompleteHIDShortcut() throws {
        let copy = try XCTUnwrap(KeyboardPassthrough.commandCombo(usage: 0x06, mods: 0x08))
        XCTAssertEqual(KeyboardPassthrough.keyEvents(for: copy, heldModifiers: 0), [
            .init(usage: 0xE3, down: true, mods: 0x08),
            .init(usage: 0x06, down: true, mods: 0x08),
            .init(usage: 0x06, down: false, mods: 0x08),
            .init(usage: 0xE3, down: false, mods: 0),
        ])
    }

    func testCommandActionRestoresHeldModifiersAndReleasesSyntheticShift() throws {
        let redo = try XCTUnwrap(KeyboardPassthrough.commandCombo(usage: 0x1D, mods: 0x0A))
        let events = KeyboardPassthrough.keyEvents(for: redo, heldModifiers: 0x80)
        XCTAssertEqual(events.first, .init(usage: 0xE1, down: true, mods: 0x82))
        XCTAssertEqual(events.filter { $0.usage == 0x1D }, [
            .init(usage: 0x1D, down: true, mods: 0x82),
            .init(usage: 0x1D, down: false, mods: 0x82),
        ])
        XCTAssertEqual(Array(events.suffix(3)), [
            .init(usage: 0xE7, down: false, mods: 0x02),
            .init(usage: 0xE1, down: false, mods: 0),
            .init(usage: 0xE7, down: true, mods: 0x80),
        ])
    }

    func testSplitViewRelayoutPreservesZoomAndVisibleCenter() {
        let oldSize = CGSize(width: 1024, height: 768)
        let newSize = CGSize(width: 507, height: 768)
        let viewport = SessionViewport(contentRect: CGRect(origin: .zero, size: oldSize),
                                       zoom: 2, offset: CGPoint(x: 0.05, y: 0.35))
        let next = SessionZoomMath.relayout(viewport: viewport, oldSize: oldSize, newSize: newSize)
        XCTAssertEqual(next.zoom, 2)
        XCTAssertEqual(next.visibleRect.midX, 0.3, accuracy: 0.01)
        XCTAssertEqual(next.visibleRect.midY, 0.6, accuracy: 0.01)
        XCTAssertEqual(next.contentRect.width, newSize.width, accuracy: 0.001)
        XCTAssertEqual(next.contentRect.width / next.contentRect.height, 4 / 3, accuracy: 0.001)
        assertWithinImage(next)

        let restored = SessionZoomMath.relayout(viewport: next, oldSize: newSize, newSize: oldSize)
        XCTAssertEqual(restored.visibleRect.midX, viewport.visibleRect.midX, accuracy: 0.0001)
        XCTAssertEqual(restored.visibleRect.midY, viewport.visibleRect.midY, accuracy: 0.0001)
    }

    func testUnzoomedRelayoutAlwaysShowsTheWholeImage() {
        var viewport = SessionViewport(contentRect: CGRect(x: 0, y: 96, width: 1024, height: 576))
        var oldSize = CGSize(width: 1024, height: 768)
        for size in [CGSize(width: 320, height: 768), CGSize(width: 507, height: 768),
                     CGSize(width: 694, height: 768), CGSize(width: 768, height: 1024),
                     CGSize(width: 640, height: 480)] {
            viewport = SessionZoomMath.relayout(viewport: viewport, oldSize: oldSize, newSize: size)
            XCTAssertEqual(viewport.zoom, 1)
            XCTAssertEqual(viewport.visibleRect, CGRect(x: 0, y: 0, width: 1, height: 1))
            XCTAssertEqual(viewport.contentRect.midX, size.width / 2, accuracy: 0.001)
            XCTAssertEqual(viewport.contentRect.midY, size.height / 2, accuracy: 0.001)
            oldSize = size
        }
    }

    func testEdgeAndTransientZeroSizeRelayoutRemainClamped() {
        let size = CGSize(width: 1024, height: 768)
        let viewport = SessionViewport(contentRect: CGRect(origin: .zero, size: size),
                                       zoom: 2, offset: CGPoint(x: 1, y: -1))
        let empty = SessionZoomMath.relayout(viewport: viewport, oldSize: size, newSize: .zero)
        XCTAssertEqual(empty.contentRect, viewport.contentRect)
        XCTAssertEqual(empty.visibleRect, viewport.visibleRect)
        let next = SessionZoomMath.relayout(viewport: empty, oldSize: .zero,
                                           newSize: CGSize(width: 507, height: 768))
        XCTAssertEqual(next.zoom, 2)
        assertWithinImage(next)
    }

    private func assertWithinImage(_ viewport: SessionViewport,
                                   file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertGreaterThanOrEqual(viewport.visibleRect.minX, 0, file: file, line: line)
        XCTAssertGreaterThanOrEqual(viewport.visibleRect.minY, 0, file: file, line: line)
        XCTAssertLessThanOrEqual(viewport.visibleRect.maxX, 1, file: file, line: line)
        XCTAssertLessThanOrEqual(viewport.visibleRect.maxY, 1, file: file, line: line)
    }
}
