import CoreGraphics
import XCTest
@testable import NativeLogic

final class PointerTests: XCTestCase {
    func testPointerPolicyRoutesAndReasons() {
        let ready = PointerLockInputs(mouseConnected: true,
                                      streaming: true,
                                      visible: true,
                                      fullScreen: true)
        XCTAssertTrue(PointerLockPolicy.wantsLock(ready))
        XCTAssertEqual(PointerLockPolicy.reason(ready), .pending)
        XCTAssertEqual(PointerLockPolicy.route(ready), .absolute)

        let split = PointerLockInputs(mouseConnected: true,
                                      streaming: true,
                                      visible: true,
                                      fullScreen: false)
        XCTAssertTrue(PointerLockPolicy.wantsLock(split))
        XCTAssertEqual(PointerLockPolicy.reason(split), .notFullScreen)

        let locked = PointerLockInputs(mouseConnected: true,
                                       streaming: true,
                                       visible: true,
                                       fullScreen: true,
                                       systemLocked: true)
        XCTAssertEqual(PointerLockPolicy.reason(locked), .locked)
        XCTAssertEqual(PointerLockPolicy.route(locked), .relative)

        let escaped = PointerLockInputs(mouseConnected: true,
                                        streaming: true,
                                        visible: true,
                                        fullScreen: true,
                                        escaped: true)
        XCTAssertFalse(PointerLockPolicy.wantsLock(escaped))
        XCTAssertEqual(PointerLockPolicy.reason(escaped), .escaped)
        XCTAssertEqual(PointerLockPolicy.route(escaped), .idle)

        XCTAssertEqual(PointerLockPolicy.reason(PointerLockInputs(escaped: true)), .noMouse)
        XCTAssertEqual(PointerLockPolicy.reason(PointerLockInputs(mouseConnected: true,
                                                                   streaming: false)),
                       .notStreaming)
        XCTAssertEqual(PointerLockPolicy.reason(PointerLockInputs(mouseConnected: true,
                                                                   streaming: true,
                                                                   visible: false)),
                       .hidden)
    }

    func testFullScreenToleranceAndRotation() {
        // Exact match, 90-degree rotation, and a 0.5-point tolerance all pass.
        XCTAssertTrue(PointerLockPolicy.isFullScreen(window: CGSize(width: 1194, height: 834),
                                                     screen: CGSize(width: 1194, height: 834)))
        XCTAssertTrue(PointerLockPolicy.isFullScreen(window: CGSize(width: 834, height: 1194),
                                                     screen: CGSize(width: 1194, height: 834)))
        XCTAssertFalse(PointerLockPolicy.isFullScreen(window: CGSize(width: 786, height: 834),
                                                      screen: CGSize(width: 1194, height: 834)))
        XCTAssertTrue(PointerLockPolicy.isFullScreen(window: CGSize(width: 1194.5, height: 834),
                                                     screen: CGSize(width: 1194, height: 834)))
        XCTAssertFalse(PointerLockPolicy.isFullScreen(window: .zero,
                                                      screen: CGSize(width: 1194, height: 834)))
        XCTAssertFalse(PointerLockPolicy.isFullScreen(window: CGSize(width: CGFloat.nan, height: 834),
                                                      screen: CGSize(width: 1194, height: 834)))
    }

    func testPointerAccelerationCurve() {
        XCTAssertEqual(PointerMotion.gain(speed: 0), 1)
        XCTAssertEqual(PointerMotion.gain(speed: 150), 1)
        XCTAssertEqual(PointerMotion.gain(speed: 825), 2)
        XCTAssertEqual(PointerMotion.gain(speed: 1500), 3)
        XCTAssertEqual(PointerMotion.gain(speed: 1e9), 3)
        XCTAssertEqual(PointerMotion.gain(speed: .nan), 1)

        var previous = PointerMotion.gain(speed: 0)
        for speed in stride(from: CGFloat(50), through: 2000, by: 50) {
            let current = PointerMotion.gain(speed: speed)
            XCTAssertGreaterThanOrEqual(current, previous)
            previous = current
        }
    }

    func testPointerMotionSlowSamplesStayOneToOne() {
        var motion = PointerMotion()
        var total: CGFloat = 0
        for index in 0..<60 {
            total += motion.viewDelta(raw: CGPoint(x: 1, y: 0), now: Double(index) / 120)
                .x
        }
        // 1 count every 1/120 s is 120 counts/s, below the 150-count/s knee.
        XCTAssertEqual(total, 60, accuracy: 0.000001)
    }

    func testPointerMotionFastSamplesUseMaximumGain() {
        var motion = PointerMotion()
        for index in 0..<60 {
            let output = motion.viewDelta(raw: CGPoint(x: 20, y: 0), now: Double(index) / 120)
            // 20 counts every 1/120 s is 2400 counts/s, capped at gain 3.
            XCTAssertEqual(output.x, 60, accuracy: 0.000001)
            XCTAssertEqual(output.y, 0, accuracy: 0.000001)
        }
    }

    func testPointerMotionResetsAfterIdleAndIgnoresInvalidRaw() {
        var motion = PointerMotion()
        XCTAssertEqual(motion.viewDelta(raw: CGPoint(x: 1, y: 0), now: 0).x, 1,
                       accuracy: 0.000001)
        XCTAssertEqual(motion.viewDelta(raw: CGPoint(x: 1, y: 0), now: 0.5).x, 1,
                       accuracy: 0.000001)
        XCTAssertEqual(motion.viewDelta(raw: CGPoint(x: CGFloat.nan, y: 0), now: 0.51), .zero)
        XCTAssertEqual(motion.viewDelta(raw: CGPoint(x: 1, y: 0), now: 0.51).x, 1,
                       accuracy: 0.000001)
    }

    func testPointerEdgeEscapeThresholdAndReset() {
        var escape = PointerEdgeEscape()
        // Eight pushes of 20 points reach the 160-point threshold exactly.
        for index in 0..<8 {
            XCTAssertEqual(escape.feed(atBottomEdge: true, viewDeltaY: 20,
                                       now: Double(index) * 0.01), index == 7)
        }
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: 20, now: 0.08))

        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: 100, now: 1))
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: -1, now: 1.1))
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: 100, now: 1.2))
        XCTAssertTrue(escape.feed(atBottomEdge: true, viewDeltaY: 60, now: 1.3))

        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: 100, now: 2))
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDeltaY: 100, now: 2.7))
        XCTAssertFalse(escape.feed(atBottomEdge: false, viewDeltaY: 1000, now: 2.8))
    }

    func testPointerScrollPhasesAndIdleEnd() {
        var scroll = PointerScrollSynth()
        // One scroll unit is 10 points; the first sample starts phase 1.
        XCTAssertEqual(scroll.feed(x: 0, y: 1, now: 0),
                       .init(dx: 0, dy: 10, phase: 1))
        XCTAssertEqual(scroll.feed(x: 0, y: 1, now: 0.01),
                       .init(dx: 0, dy: 10, phase: 2))
        XCTAssertNil(scroll.idle(now: 0.05))
        XCTAssertEqual(scroll.idle(now: 0.2), .init(dx: 0, dy: 0, phase: 3))
        XCTAssertNil(scroll.idle(now: 0.3))
        XCTAssertNil(scroll.feed(x: 0, y: 0, now: 0.4))
        XCTAssertNil(scroll.reset())
    }

    func testPointerButtonsMapMasksAndAuxiliaryButtons() {
        XCTAssertEqual(PointerButtons.fromButtonMask(1), 0)
        XCTAssertEqual(PointerButtons.fromButtonMask(2), 1)
        XCTAssertEqual(PointerButtons.fromButtonMask(4), 2)
        XCTAssertEqual(PointerButtons.fromButtonMask(3), 0)
        XCTAssertNil(PointerButtons.fromButtonMask(0))
        XCTAssertNil(PointerButtons.fromButtonMask(32))
        XCTAssertEqual(PointerButtons.auxiliary(index: 0), 3)
        XCTAssertEqual(PointerButtons.auxiliary(index: 1), 4)
        XCTAssertNil(PointerButtons.auxiliary(index: 2))
    }
}
