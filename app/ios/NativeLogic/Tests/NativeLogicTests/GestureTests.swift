import CoreGraphics
import XCTest
@testable import NativeLogic

final class GestureTests: XCTestCase {
    func testAcceleration() {
        XCTAssertEqual(TrackpadAccel.gain(speed: 0), 1)
        XCTAssertEqual(TrackpadAccel.gain(speed: 450), 1.5)
        XCTAssertEqual(TrackpadAccel.gain(speed: 1800), 3)
        XCTAssertEqual(TrackpadAccel.gain(speed: 4000), 3)
    }

    func testCursorMovementAndClamping() {
        var cursor = TrackpadCursorModel()
        let size = CGSize(width: 1000, height: 500)
        cursor.apply(viewDelta: CGPoint(x: 100, y: 50), contentSize: size, gain: 1)
        assertPoint(cursor.position, CGPoint(x: 0.6, y: 0.6))
        cursor.apply(viewDelta: CGPoint(x: -2000, y: 1000), contentSize: size, gain: 3)
        XCTAssertEqual(cursor.position, CGPoint(x: 0, y: 1))
        cursor.apply(viewDelta: CGPoint(x: 20, y: 20), contentSize: .zero, gain: 1)
        XCTAssertEqual(cursor.position, CGPoint(x: 0, y: 1))
    }

    func testClickCountAndReset() {
        var clicks = TrackpadClickCounter()
        let point = CGPoint(x: 0.5, y: 0.5)
        XCTAssertEqual(clicks.register(position: point, time: 0), 1)
        XCTAssertEqual(clicks.register(position: point, time: 0.1), 2)
        XCTAssertEqual(clicks.register(position: point, time: 0.2), 3)
        XCTAssertEqual(clicks.register(position: point, time: 0.3), 3)
        XCTAssertEqual(clicks.register(position: point, time: 0.8), 1)
        XCTAssertEqual(clicks.register(position: CGPoint(x: 0.52, y: 0.5), time: 0.9), 1)
    }

    func testMomentumDecaysAndEnds() {
        var momentum = TrackpadScrollMomentum()
        momentum.start(velocity: CGPoint(x: 600, y: 0))
        var previous: CGFloat = 600
        var frame = 0
        while momentum.isActive && frame < 121 {
            let step = momentum.step(dt: 1 / 60)
            XCTAssertEqual(momentum.velocity.x, previous * 0.95, accuracy: 0.00001)
            XCTAssertEqual(step.dx, previous * 2 / 60, accuracy: 0.00001)
            XCTAssertEqual(step.dy, 0)
            if step.finished {
                XCTAssertLessThan(momentum.velocity.x, 20)
                XCTAssertEqual(step.phase, 3)
            } else {
                XCTAssertEqual(step.phase, frame == 0 ? 1 : 2)
            }
            previous = momentum.velocity.x
            frame += 1
        }
        XCTAssertFalse(momentum.isActive)
        XCTAssertGreaterThan(frame, 1)
        XCTAssertLessThan(frame, 120)
    }

    func testMomentumThresholdDurationAndInterruption() {
        var momentum = TrackpadScrollMomentum()
        momentum.start(velocity: CGPoint(x: 50, y: 0))
        XCTAssertFalse(momentum.isActive)
        XCTAssertTrue(momentum.step(dt: 1 / 60).finished)
        momentum.start(velocity: CGPoint(x: 100, y: 0))
        XCTAssertFalse(momentum.isActive)
        momentum.start(velocity: CGPoint(x: 100_000, y: 0))
        let end = momentum.step(dt: 2.5)
        XCTAssertTrue(end.finished)
        XCTAssertEqual(end.phase, 3)
        momentum.start(velocity: CGPoint(x: 0, y: -600))
        XCTAssertLessThan(momentum.step(dt: 1 / 60).dy, 0)
        XCTAssertTrue(momentum.stop())
        XCTAssertFalse(momentum.isActive)
        XCTAssertFalse(momentum.stop())
    }

    func testMomentumDecayIsIndependentOfFrameRate() {
        var slow = TrackpadScrollMomentum()
        var fast = TrackpadScrollMomentum()
        slow.start(velocity: CGPoint(x: 600, y: 300))
        fast.start(velocity: CGPoint(x: 600, y: 300))
        _ = slow.step(dt: 1 / 30)
        _ = fast.step(dt: 1 / 60)
        _ = fast.step(dt: 1 / 60)
        assertPoint(slow.velocity, fast.velocity)
    }

    func testMomentumEndsAfter120FramesEvenAtHighSpeed() {
        var momentum = TrackpadScrollMomentum()
        momentum.start(velocity: CGPoint(x: 100_000, y: 0))
        for _ in 0..<119 {
            XCTAssertFalse(momentum.step(dt: 1 / 60).finished)
        }
        let end = momentum.step(dt: 1 / 60)
        XCTAssertTrue(end.finished)
        XCTAssertEqual(end.phase, 3)
    }

    func testPointerThrottleKeepsLatestAndFlushesOnce() {
        var throttle = TrackpadPointerThrottle()
        let first = CGPoint(x: 0.1, y: 0.1)
        let last = CGPoint(x: 0.3, y: 0.3)
        XCTAssertEqual(throttle.offer(first, now: 0), first)
        XCTAssertNil(throttle.offer(CGPoint(x: 0.2, y: 0.2), now: 0.0005))
        XCTAssertNil(throttle.offer(last, now: 0.001))
        XCTAssertEqual(throttle.flush(now: 0.001), last)
        XCTAssertNil(throttle.flush(now: 0.001))
    }

    func testPointerThrottleAllows120HzAndReplacesPendingSample() {
        var throttle = TrackpadPointerThrottle()
        let point = CGPoint(x: 0.5, y: 0.5)
        XCTAssertEqual(throttle.offer(.zero, now: 0), .zero)
        XCTAssertNil(throttle.offer(.zero, now: 0.001))
        XCTAssertEqual(throttle.offer(point, now: 1 / 120), point)
        XCTAssertNil(throttle.flush(now: 1 / 120))
        XCTAssertEqual(throttle.offer(point, now: 2 / 120), point)
    }

    func testHostCursorArbitration() {
        var arbiter = TrackpadHostCursorArbiter()
        XCTAssertTrue(arbiter.shouldAcceptHost(at: 0))
        arbiter.recordLocalMove(at: 1)
        XCTAssertFalse(arbiter.shouldAcceptHost(at: 1.1))
        XCTAssertTrue(arbiter.shouldAcceptHost(at: 1.25))
        arbiter.recordLocalMove(at: 1.3)
        XCTAssertFalse(arbiter.shouldAcceptHost(at: 1.4))
    }

    func testViewportUnzoomedMapping() {
        let viewport = SessionViewport(contentRect: CGRect(x: 100, y: 0, width: 800, height: 450))
        let center = CGPoint(x: 0.5, y: 0.5)
        assertPoint(viewport.normalized(fromView: CGPoint(x: 500, y: 225)), center)
        assertPoint(viewport.viewPoint(fromNormalized: center), CGPoint(x: 500, y: 225))
        assertPoint(viewport.normalizedDelta(fromViewDelta: CGPoint(x: 80, y: 45)), CGPoint(x: 0.1, y: 0.1))
    }

    func testViewportZoomClampsVisibleOffsetAndScalesDeltas() {
        var viewport = SessionViewport(contentRect: CGRect(x: 100, y: 0, width: 800, height: 450),
                                       zoom: 2, offset: CGPoint(x: 0.25, y: 0.25))
        let point = CGPoint(x: 500, y: 225)
        assertPoint(viewport.normalized(fromView: point), CGPoint(x: 0.5, y: 0.5))
        assertPoint(viewport.viewPoint(fromNormalized: viewport.normalized(fromView: point)), point)
        assertPoint(viewport.normalizedDelta(fromViewDelta: CGPoint(x: 80, y: 45)), CGPoint(x: 0.05, y: 0.05))
        viewport.offset = CGPoint(x: 1, y: -1)
        XCTAssertEqual(viewport.offset, CGPoint(x: 0.5, y: 0))
        viewport.zoom = 0
        XCTAssertEqual(viewport.zoom, 1)
        XCTAssertEqual(viewport.offset, .zero)
    }

    private func assertPoint(_ actual: CGPoint, _ expected: CGPoint,
                             file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.x, expected.x, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.y, expected.y, accuracy: 0.000001, file: file, line: line)
    }
}
