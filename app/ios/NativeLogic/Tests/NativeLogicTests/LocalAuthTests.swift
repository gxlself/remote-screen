import XCTest
@testable import NativeLogic

final class LocalAuthTests: XCTestCase {
    func testCancellationErrors() {
        for code in [-2, -4, -9] {
            XCTAssertEqual(
                LocalAuthErrorMap.result(
                    code: code,
                    canBiometrics: true,
                    canPasscode: true
                ),
                "cancelled"
            )
        }
    }

    func testAuthenticationFailures() {
        for code in [-1, -8] {
            XCTAssertEqual(
                LocalAuthErrorMap.result(
                    code: code,
                    canBiometrics: true,
                    canPasscode: true
                ),
                "failed"
            )
        }
    }

    func testPasscodeNotSetIsUnavailable() {
        XCTAssertEqual(
            LocalAuthErrorMap.result(
                code: -5,
                canBiometrics: false,
                canPasscode: false
            ),
            "unavailable"
        )
    }

    func testBiometryAvailabilityDependsOnPasscode() {
        XCTAssertEqual(
            LocalAuthErrorMap.result(
                code: -6,
                canBiometrics: false,
                canPasscode: false
            ),
            "unavailable"
        )
        XCTAssertEqual(
            LocalAuthErrorMap.result(
                code: -6,
                canBiometrics: false,
                canPasscode: true
            ),
            "failed"
        )
    }

    func testUnknownErrorIsFailure() {
        XCTAssertEqual(
            LocalAuthErrorMap.result(
                code: -999,
                canBiometrics: false,
                canPasscode: false
            ),
            "failed"
        )
    }

    func testBiometryNames() {
        XCTAssertEqual(LocalAuthBiometry.name(rawType: 2), "faceID")
        XCTAssertEqual(LocalAuthBiometry.name(rawType: 0), "none")
    }
}
