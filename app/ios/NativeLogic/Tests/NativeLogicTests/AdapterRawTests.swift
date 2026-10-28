import XCTest
@testable import NativeLogic

final class AdapterRawTests: XCTestCase {
    func testPacingFirstReadyImmediate() {
        var pacing = AdapterRawPacing()
        XCTAssertEqual(pacing.ready(seq: 1, now: 10), 0)
        XCTAssertNil(pacing.ready(seq: 2, now: 10.001))
    }

    func testPacingBeginReturnsLatest() {
        var pacing = AdapterRawPacing()
        _ = pacing.ready(seq: 1, now: 10)
        _ = pacing.ready(seq: 3, now: 10.001)
        _ = pacing.ready(seq: 2, now: 10.002)
        XCTAssertEqual(pacing.begin(now: 10.003), 3)
    }

    func testPacingEndIdle() {
        var pacing = AdapterRawPacing()
        _ = pacing.ready(seq: 3, now: 9)
        XCTAssertEqual(pacing.begin(now: 10), 3)
        XCTAssertNil(pacing.end(pulledSeq: 3, now: 10.004))
        let delay = pacing.ready(seq: 4, now: 10.005)
        XCTAssertEqual(delay ?? -1, 10 + 1.0 / 60 - 10.005, accuracy: 1e-9)
    }

    func testPacingEndBehind() {
        var pacing = AdapterRawPacing()
        _ = pacing.ready(seq: 4, now: 9)
        _ = pacing.begin(now: 10)
        XCTAssertNil(pacing.ready(seq: 5, now: 10.001))
        XCTAssertEqual(pacing.end(pulledSeq: 4, now: 10.002) ?? -1, 1.0 / 60 - 0.002, accuracy: 1e-9)

        _ = pacing.begin(now: 10.02)
        XCTAssertEqual(pacing.end(pulledSeq: 4, now: 10.04) ?? -1, 0, accuracy: 1e-9)
    }

    func testPacingEndZero() {
        var pacing = AdapterRawPacing()
        _ = pacing.ready(seq: 2, now: 10)
        _ = pacing.begin(now: 10)
        XCTAssertNil(pacing.ready(seq: 5, now: 10.0005))
        XCTAssertNil(pacing.end(pulledSeq: 0, now: 10.001))
        XCTAssertEqual(pacing.ready(seq: 6, now: 10.002) ?? -1, 1.0 / 60 - 0.002, accuracy: 1e-9)
    }

    func testPacingReset() {
        var pacing = AdapterRawPacing()
        _ = pacing.ready(seq: 8, now: 10)
        _ = pacing.begin(now: 10)
        pacing.reset()
        XCTAssertEqual(pacing.ready(seq: 1, now: 10), 0)
        XCTAssertEqual(pacing.begin(now: 10), 1)
    }

    func testRingRotates() {
        var ring = AdapterRawRing()
        let slots = (0..<4).map { _ in ring.next(width: 4, height: 3) }
        XCTAssertEqual(slots.map(\.index), [0, 1, 2, 0])
        XCTAssertEqual(slots.prefix(3).map(\.needsAlloc), [true, true, true])
    }

    func testRingSinceSeq() {
        var ring = AdapterRawRing()
        _ = ring.next(width: 4, height: 3)
        ring.copied(index: 0, seq: 7)
        _ = ring.next(width: 4, height: 3)
        _ = ring.next(width: 4, height: 3)
        let slot = ring.next(width: 4, height: 3)
        XCTAssertEqual(slot, AdapterRawSlot(index: 0, sinceSeq: 7, needsAlloc: false))
    }

    func testRingResize() {
        var ring = AdapterRawRing()
        _ = ring.next(width: 4, height: 3)
        ring.copied(index: 0, seq: 7)
        _ = ring.next(width: 4, height: 3)
        _ = ring.next(width: 4, height: 3)
        let slot = ring.next(width: 8, height: 6)
        XCTAssertEqual(slot, AdapterRawSlot(index: 0, sinceSeq: 0, needsAlloc: true))
    }

    func testRingInvalidate() {
        var ring = AdapterRawRing()
        _ = ring.next(width: 4, height: 3)
        _ = ring.next(width: 4, height: 3)
        ring.copied(index: 1, seq: 7)
        ring.copied(index: 1, seq: 0)
        _ = ring.next(width: 4, height: 3)
        _ = ring.next(width: 4, height: 3)
        let slot = ring.next(width: 4, height: 3)
        XCTAssertEqual(slot, AdapterRawSlot(index: 1, sinceSeq: 0, needsAlloc: true))
        ring.copied(index: 1, seq: 9)
        _ = ring.next(width: 4, height: 3)
        _ = ring.next(width: 4, height: 3)
        XCTAssertEqual(ring.next(width: 4, height: 3), AdapterRawSlot(index: 1, sinceSeq: 9, needsAlloc: false))
    }

    func testRingReset() {
        var ring = AdapterRawRing()
        for index in 0..<3 {
            _ = ring.next(width: 4, height: 3)
            ring.copied(index: index, seq: UInt64(index + 1))
        }
        _ = ring.next(width: 4, height: 3)
        ring.reset()
        let slots = (0..<3).map { _ in ring.next(width: 4, height: 3) }
        XCTAssertEqual(slots.map(\.index), [0, 1, 2])
        XCTAssertEqual(slots.map(\.sinceSeq), [0, 0, 0])
        XCTAssertTrue(slots.allSatisfy(\.needsAlloc))
    }
}
