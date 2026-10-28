import CryptoKit
import Foundation
import XCTest
@testable import NativeLogic

final class RecordingTests: XCTestCase {
    private let fileKey = Data((0..<RecordingKeyWrap.keyLength).map(UInt8.init))
    private let master = SymmetricKey(data: Data(repeating: 0x11, count: RecordingKeyWrap.keyLength))

    func testWrapAndUnwrapRoundTripAndRandomNonce() throws {
        let first = try RecordingKeyWrap.wrap(fileKey, master: master)
        let second = try RecordingKeyWrap.wrap(fileKey, master: master)

        XCTAssertEqual(first.count, RecordingKeyWrap.wrappedLength)
        XCTAssertEqual(first.first, RecordingKeyWrap.version)
        XCTAssertNotEqual(first, second)
        XCTAssertEqual(try RecordingKeyWrap.unwrap(first, master: master), fileKey)
    }

    func testTamperingIsRejectedWithVersionDistinguished() throws {
        let wrapped = try RecordingKeyWrap.wrap(fileKey, master: master)
        for index in [0, 5, 20, 60] {
            var tampered = wrapped
            tampered[index] = tampered[index] ^ 0xFF

            XCTAssertThrowsError(try RecordingKeyWrap.unwrap(tampered, master: master)) { error in
                if index == 0 {
                    XCTAssertEqual(error as? RecordingKeyWrapError, .badVersion)
                } else {
                    XCTAssertEqual(error as? RecordingKeyWrapError, .rejected)
                }
            }
        }
    }

    func testDifferentMasterIsRejected() throws {
        let wrapped = try RecordingKeyWrap.wrap(fileKey, master: master)
        let otherMaster = SymmetricKey(data: Data(repeating: 0x22, count: RecordingKeyWrap.keyLength))

        XCTAssertThrowsError(try RecordingKeyWrap.unwrap(wrapped, master: otherMaster)) { error in
            XCTAssertEqual(error as? RecordingKeyWrapError, .rejected)
        }
    }

    func testInvalidLengthsAreRejected() {
        XCTAssertThrowsError(
            try RecordingKeyWrap.wrap(
                Data(repeating: 0, count: RecordingKeyWrap.keyLength - 1),
                master: master
            )
        ) { error in
            XCTAssertEqual(error as? RecordingKeyWrapError, .badLength)
        }

        XCTAssertThrowsError(
            try RecordingKeyWrap.unwrap(
                Data(repeating: 0, count: RecordingKeyWrap.wrappedLength - 1),
                master: master
            )
        ) { error in
            XCTAssertEqual(error as? RecordingKeyWrapError, .badLength)
        }
    }

    func testRecordingWrapCall() {
        let wrap = RecordingWrapCall.handle(
            op: 1,
            input: fileKey,
            capacity: 256,
            master: { master }
        )
        XCTAssertEqual(wrap.0, RecordingWrapCall.ok)
        XCTAssertEqual(wrap.1.count, RecordingKeyWrap.wrappedLength)

        let unwrap = RecordingWrapCall.handle(
            op: 2,
            input: wrap.1,
            capacity: 256,
            master: { master }
        )
        XCTAssertEqual(unwrap.0, RecordingWrapCall.ok)
        XCTAssertEqual(unwrap.1, fileKey)

        let unavailable = RecordingWrapCall.handle(
            op: 1,
            input: fileKey,
            capacity: 256,
            master: { nil }
        )
        XCTAssertEqual(unavailable.0, RecordingWrapCall.unavailable)
        XCTAssertTrue(unavailable.1.isEmpty)

        let tooSmall = RecordingWrapCall.handle(
            op: 1,
            input: fileKey,
            capacity: 10,
            master: { master }
        )
        XCTAssertEqual(tooSmall.0, RecordingWrapCall.tooSmall)
        XCTAssertTrue(tooSmall.1.isEmpty)

        let unknown = RecordingWrapCall.handle(
            op: 9,
            input: fileKey,
            capacity: 256,
            master: { master }
        )
        XCTAssertEqual(unknown.0, RecordingWrapCall.rejected)
        XCTAssertTrue(unknown.1.isEmpty)

        let badInput = RecordingWrapCall.handle(
            op: 1,
            input: Data(repeating: 0, count: 16),
            capacity: 256,
            master: { master }
        )
        XCTAssertEqual(badInput.0, RecordingWrapCall.rejected)
        XCTAssertTrue(badInput.1.isEmpty)
    }
}
