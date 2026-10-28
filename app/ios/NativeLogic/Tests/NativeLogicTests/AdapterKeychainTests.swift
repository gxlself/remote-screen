import XCTest
@testable import NativeLogic

final class AdapterKeychainTests: XCTestCase {
    func testAccount() {
        XCTAssertEqual(AdapterKeychainLogic.account(fromKeyBytes: Array("vnc:h1".utf8)), "vnc:h1")
        XCTAssertNil(AdapterKeychainLogic.account(fromKeyBytes: []))
        XCTAssertNil(AdapterKeychainLogic.account(fromKeyBytes: [UInt8](repeating: 0x61, count: 301)))
        XCTAssertNil(AdapterKeychainLogic.account(fromKeyBytes: [0xC3, 0x28]))
        XCTAssertNotNil(AdapterKeychainLogic.account(fromKeyBytes: [UInt8](repeating: 0x61, count: 300)))

        let multibyte = String(repeating: "é", count: 150)
        XCTAssertEqual(AdapterKeychainLogic.account(fromKeyBytes: Array(multibyte.utf8)), multibyte)
        XCTAssertNil(AdapterKeychainLogic.account(fromKeyBytes: Array((multibyte + "é").utf8)))
    }

    func testGetCode() {
        XCTAssertEqual(AdapterKeychainLogic.getCode(status: 0), 0)
        XCTAssertEqual(AdapterKeychainLogic.getCode(status: -25300), 1)
        XCTAssertEqual(AdapterKeychainLogic.getCode(status: -25308), -25308)
        XCTAssertEqual(AdapterKeychainLogic.getCode(status: 1), -1)
    }

    func testFits() {
        XCTAssertTrue(AdapterKeychainLogic.fits(valueCount: 4096, capacity: 4096))
        XCTAssertFalse(AdapterKeychainLogic.fits(valueCount: 4097, capacity: 8192))
        XCTAssertFalse(AdapterKeychainLogic.fits(valueCount: 10, capacity: 9))
        XCTAssertTrue(AdapterKeychainLogic.fits(valueCount: 0, capacity: 0))
        XCTAssertFalse(AdapterKeychainLogic.fits(valueCount: 0, capacity: -1))
        XCTAssertFalse(AdapterKeychainLogic.fits(valueCount: -1, capacity: 4096))
    }

    func testDeleteCode() {
        XCTAssertEqual(AdapterKeychainLogic.deleteCode(status: 0), 0)
        XCTAssertEqual(AdapterKeychainLogic.deleteCode(status: -25300), 0)
        XCTAssertEqual(AdapterKeychainLogic.deleteCode(status: -34018), -34018)
        XCTAssertEqual(AdapterKeychainLogic.deleteCode(status: 1), -1)
    }

    func testPutSteps() {
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .update, status: 0))
        XCTAssertEqual(AdapterKeychainLogic.nextPutStep(after: .update, status: -25300), .add)
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .add, status: 0))
        XCTAssertEqual(AdapterKeychainLogic.nextPutStep(after: .add, status: -25299), .updateAgain)
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .updateAgain, status: -25300))
        XCTAssertEqual(AdapterKeychainLogic.putCode(lastStatus: -25300), -25300)
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .update, status: -25308))
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .add, status: -25308))
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .updateAgain, status: -25299))
        XCTAssertNil(AdapterKeychainLogic.nextPutStep(after: .updateAgain, status: 0))
        XCTAssertEqual(AdapterKeychainLogic.putCode(lastStatus: 0), 0)
        XCTAssertEqual(AdapterKeychainLogic.putCode(lastStatus: 1), -1)
    }

    func testShouldPurge() {
        XCTAssertTrue(AdapterKeychainLogic.shouldPurge(installedFlag: false))
        XCTAssertFalse(AdapterKeychainLogic.shouldPurge(installedFlag: true))
    }
}
