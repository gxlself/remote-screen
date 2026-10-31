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

    func testPointerEdgeEscapeNeedsSustainedPush() {
        var escape = PointerEdgeEscape()
        let down = { (dy: CGFloat) in CGVector(dx: 0, dy: dy) }
        // A flick onto the taskbar overshoots far past the threshold but is
        // over long before the minimum duration.
        for index in 0..<8 {
            XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(60), now: Double(index) * 0.01))
        }
        // Pushes of 10 points every 20 ms: 260 points after 0.5 s.
        for index in 0...25 {
            XCTAssertEqual(escape.feed(atBottomEdge: true, viewDelta: down(10), now: 1 + Double(index) / 50),
                           index == 25)
        }
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(10), now: 1.52))
    }

    func testPointerEdgeEscapeStartsOver() {
        var escape = PointerEdgeEscape()
        let down = { (dy: CGFloat) in CGVector(dx: 0, dy: dy) }
        // A pause longer than maxGap starts the push over.
        for index in 0...20 {
            XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(20), now: Double(index) / 50))
        }
        for index in 0...15 {
            XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(20), now: 0.6 + Double(index) / 50))
        }
        // Moving along the edge, moving up and leaving the edge all start over.
        let interruptions = [(true, CGVector(dx: 30, dy: 10)), (true, down(-1)), (false, down(20))]
        for (atBottomEdge, delta) in interruptions {
            let start = 2.0
            for index in 0...20 {
                XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(20), now: start + Double(index) / 50))
            }
            XCTAssertFalse(escape.feed(atBottomEdge: atBottomEdge, viewDelta: delta, now: start + 0.42))
            for index in 0...20 {
                XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: down(20), now: start + 0.44 + Double(index) / 50))
            }
            escape.reset()
        }
        XCTAssertFalse(escape.feed(atBottomEdge: true, viewDelta: CGVector(dx: CGFloat.nan, dy: 10), now: 3))
    }

    func testPointerScrollPhasesAndIdleEnd() {
        var scroll = PointerScrollSynth()
        let unit = PointerScrollSynth.pointsPerUnit
        // The first sample starts phase 1.
        XCTAssertEqual(scroll.feed(x: 0, y: 2, now: 0),
                       .init(dx: 0, dy: 2 * unit, phase: 1))
        XCTAssertEqual(scroll.feed(x: 0, y: 1, now: 0.01),
                       .init(dx: 0, dy: unit, phase: 2))
        XCTAssertNil(scroll.idle(now: 0.05))
        let end = scroll.idle(now: 0.2)
        XCTAssertEqual(end?.phase, 3)
        XCTAssertEqual(end?.dy, 0)
        XCTAssertNil(scroll.idle(now: 0.3))
        XCTAssertNil(scroll.feed(x: 0, y: 0, now: 0.4))
        XCTAssertNil(scroll.reset())
        XCTAssertEqual(scroll.summary.samples, 2)
        XCTAssertEqual(scroll.summary.total.dy, 3 * unit)
        XCTAssertEqual(scroll.summary.peak, 2)
        XCTAssertEqual(scroll.summary.duration, 0.01, accuracy: 1e-9)
    }

    func testPointerScrollKeepsToOneAxis() {
        var scroll = PointerScrollSynth()
        let unit = PointerScrollSynth.pointsPerUnit
        // Held back until it has gone far enough to tell, then sent at once.
        XCTAssertEqual(scroll.feed(x: 0.3, y: 1, now: 0), .init(dx: 0, dy: 0, phase: 1))
        XCTAssertEqual(scroll.feed(x: 0.2, y: 1.5, now: 0.01), .init(dx: 0, dy: 2.5 * unit, phase: 2))
        // Sideways drift later in an up/down swipe is dropped.
        XCTAssertEqual(scroll.feed(x: 2, y: 0.5, now: 0.02), .init(dx: 0, dy: unit / 2, phase: 2))
        XCTAssertEqual(scroll.summary.peak, 2)
        _ = scroll.idle(now: 0.2)

        XCTAssertEqual(scroll.feed(x: -3, y: 0.2, now: 1), .init(dx: -3 * unit, dy: 0, phase: 1))
        XCTAssertEqual(scroll.feed(x: 0, y: 1, now: 1.01), .init(dx: 0, dy: 0, phase: 2))
    }

    func testPointerScrollFlickLeavesItsSpeed() {
        var scroll = PointerScrollSynth()
        let unit = PointerScrollSynth.pointsPerUnit
        // One unit every 10 ms: 100 units a second.
        for step in 0..<10 {
            _ = scroll.feed(x: 0, y: -1, now: Double(step) * 0.01)
        }
        let end = scroll.idle(now: 0.25)
        XCTAssertEqual(end?.velocity.dx ?? 1, 0, accuracy: 1e-6)
        XCTAssertEqual(end?.velocity.dy ?? 0, -100 * unit, accuracy: 1)
        XCTAssertEqual(scroll.summary.samples, 10)
        XCTAssertEqual(scroll.summary.total.dy, -10 * unit)
    }

    func testPointerScrollSlowingDownLeavesLittleSpeed() {
        var scroll = PointerScrollSynth()
        for step in 0..<10 {
            _ = scroll.feed(x: 0, y: 2, now: Double(step) * 0.01)
        }
        // The fingers slow to a crawl before they stop: 3 units a second.
        for step in 0..<10 {
            _ = scroll.feed(x: 0, y: 0.03, now: 0.1 + Double(step) * 0.01)
        }
        let end = scroll.idle(now: 0.4)
        XCTAssertLessThan(abs(end?.velocity.dy ?? .infinity), 4 * PointerScrollSynth.pointsPerUnit)
    }

    func testPointerScrollSpeedNeedsTwoSamplesAndIsCapped() {
        var scroll = PointerScrollSynth()
        _ = scroll.feed(x: 0, y: 5, now: 0)
        XCTAssertEqual(scroll.idle(now: 0.2)?.velocity, .zero)

        _ = scroll.feed(x: 0, y: 500, now: 1)
        _ = scroll.feed(x: 0, y: 500, now: 1.001)
        XCTAssertEqual(scroll.idle(now: 1.2)?.velocity.dy ?? 0, PointerScrollSynth.maxVelocity, accuracy: 1e-6)

        // A cancelled scroll does not fling.
        _ = scroll.feed(x: 0, y: 1, now: 2)
        _ = scroll.feed(x: 0, y: 1, now: 2.01)
        XCTAssertEqual(scroll.reset(), .init(dx: 0, dy: 0, phase: 3))
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
