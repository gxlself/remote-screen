import XCTest
@testable import NativeLogic

final class ScannerTests: XCTestCase {
    func testThrottleSuppressesRepeatedTextForTwoSeconds() {
        var throttle = QrScanThrottle()

        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 0))
        XCTAssertFalse(throttle.accept("rsp://pair?a", nowMs: 1_999))
        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 2_001))
        XCTAssertTrue(throttle.accept("rsp://pair?b", nowMs: 2_002))
    }

    func testThrottleKeepsSeparateWindowsForDifferentCodes() {
        var throttle = QrScanThrottle()

        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 0))
        XCTAssertTrue(throttle.accept("rsp://pair?b", nowMs: 1))
        XCTAssertFalse(throttle.accept("rsp://pair?a", nowMs: 2))
        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 2_001))
    }

    func testThrottleRecoversAfterClockMovesBackwards() {
        var throttle = QrScanThrottle()

        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 5_000))
        XCTAssertTrue(throttle.accept("rsp://pair?a", nowMs: 1_000))
        XCTAssertFalse(throttle.accept("rsp://pair?a", nowMs: 1_001))
    }

    func testPermissionDecisionPrioritizesMissingDevice() {
        XCTAssertEqual(
            ScannerPermission.decide(.authorized, hasDevice: true),
            .start
        )
        XCTAssertEqual(
            ScannerPermission.decide(.notDetermined, hasDevice: true),
            .request
        )
        XCTAssertEqual(
            ScannerPermission.decide(.denied, hasDevice: true),
            .denied
        )
        XCTAssertEqual(
            ScannerPermission.decide(.restricted, hasDevice: true),
            .restricted
        )
        let statuses: [ScannerPermissionStatus] = [
            .authorized, .notDetermined, .denied, .restricted,
        ]
        for status in statuses {
            XCTAssertEqual(
                ScannerPermission.decide(status, hasDevice: false),
                .unavailable
            )
        }
    }
}
