import CoreGraphics
import XCTest
@testable import NativeLogic

final class GesturePrefsTests: XCTestCase {
    func testDefaults() {
        let prefs = SessionGesturePrefs()
        XCTAssertEqual(prefs.cursorSpeed, 1.4)
        XCTAssertTrue(prefs.naturalScroll)
        XCTAssertTrue(prefs.haptics)
    }

    func testCursorSpeedClampsToSupportedRange() {
        XCTAssertEqual(SessionGesturePrefs(cursorSpeed: 5).cursorSpeed, 2.4)
        XCTAssertEqual(SessionGesturePrefs(cursorSpeed: 0).cursorSpeed, 0.6)
        XCTAssertEqual(SessionGesturePrefs(cursorSpeed: -1).cursorSpeed, 0.6)
        XCTAssertEqual(SessionGesturePrefs(cursorSpeed: 2).cursorSpeed, 2)
    }

    func testNonFiniteCursorSpeedUsesDefault() {
        let speeds: [CGFloat] = [.nan, .infinity, -.infinity]
        for speed in speeds {
            XCTAssertEqual(SessionGesturePrefs(cursorSpeed: speed).cursorSpeed, 1.4)
        }
    }

    func testDefaultPointerDeltaPreservesExistingTuning() {
        let delta = CGVector(dx: 2.8, dy: -7)
        assertVector(SessionGesturePrefs(cursorSpeed: 1.4).pointerDelta(delta), delta)
    }

    func testPointerDeltaScalesRelativeToDefaultSpeed() {
        let delta = CGVector(dx: 2.8, dy: -1.4)
        assertVector(SessionGesturePrefs(cursorSpeed: 1).pointerDelta(delta), CGVector(dx: 2, dy: -1))
        assertVector(SessionGesturePrefs(cursorSpeed: 2).pointerDelta(delta), CGVector(dx: 4, dy: -2))
        assertVector(SessionGesturePrefs(cursorSpeed: 2.8).pointerDelta(delta), CGVector(dx: 4.8, dy: -2.4))
    }

    func testNaturalScrollPreservesBothAxes() {
        let delta = CGVector(dx: 12, dy: -8)
        assertVector(SessionGesturePrefs(naturalScroll: true).scrollDelta(delta), delta)
    }

    func testDisabledNaturalScrollReversesBothAxes() {
        assertVector(SessionGesturePrefs(naturalScroll: false).scrollDelta(CGVector(dx: 12, dy: -8)),
                     CGVector(dx: -12, dy: 8))
    }

    func testScrollIsIndependentOfCursorSpeedAndHaptics() {
        let delta = CGVector(dx: -6, dy: 4)
        let prefs = SessionGesturePrefs(cursorSpeed: 2.4, naturalScroll: true, haptics: false)
        assertVector(prefs.scrollDelta(delta), delta)
        XCTAssertFalse(prefs.haptics)
    }

    private func assertVector(_ actual: CGVector, _ expected: CGVector,
                              file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.dx, expected.dx, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.dy, expected.dy, accuracy: 0.000001, file: file, line: line)
    }
}
