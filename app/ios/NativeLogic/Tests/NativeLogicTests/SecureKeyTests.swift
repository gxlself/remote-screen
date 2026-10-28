import CryptoKit
import XCTest
@testable import NativeLogic

final class SecureKeyTests: XCTestCase {
    func testSpkiMatchesCryptoKitDerRepresentation() throws {
        let key = P256.Signing.PrivateKey()
        XCTAssertEqual(try SecureKeySpki.der(x963: key.publicKey.x963Representation), key.publicKey.derRepresentation)
        XCTAssertEqual(key.publicKey.derRepresentation.count, 91)

        XCTAssertThrowsError(try SecureKeySpki.der(x963: Data(repeating: 0x04, count: 64)))
        XCTAssertThrowsError(try SecureKeySpki.der(x963: Data([0x02] + Array(repeating: 0, count: 64))))
    }

    func testFingerprintAndDeviceId() {
        let fp = Data((0..<32).map(UInt8.init))
        XCTAssertEqual(SecureKeySpki.formatFingerprint(fp), "SHA256 " + (0..<32).map { String(format: "%02X", $0) }.joined(separator: ":"))
        XCTAssertEqual(SecureKeySpki.deviceId(Data((0..<32).map(UInt8.init))), "aaaqeayeaudaocaj")
    }

    func testSignBuffer() {
        let signature = Data(repeating: 0xAB, count: 70)
        var output = [UInt8](repeating: 0, count: 72)
        var length = output.count
        let result = output.withUnsafeMutableBufferPointer { buffer in
            SecureKeySignBuffer.write(signature, to: buffer.baseAddress, length: &length)
        }
        XCTAssertEqual(result, 0)
        XCTAssertEqual(length, 70)
        XCTAssertEqual(Data(output.prefix(length)), signature)

        var shortOutput = [UInt8](repeating: 0, count: 10)
        var shortLength = shortOutput.count
        let shortResult = shortOutput.withUnsafeMutableBufferPointer { buffer in
            SecureKeySignBuffer.write(signature, to: buffer.baseAddress, length: &shortLength)
        }
        XCTAssertEqual(shortResult, -2)
        XCTAssertEqual(shortLength, 70)
        XCTAssertEqual(SecureKeySignBuffer.write(signature, to: nil, length: &length), -3)
        XCTAssertEqual(SecureKeySignBuffer.write(signature, to: output.withUnsafeMutableBufferPointer { $0.baseAddress }, length: nil), -3)
    }

    func testRotation() {
        XCTAssertEqual(SecureKeyRotation.decide(installedMarker: true, keyExists: true), .useExisting)
        XCTAssertEqual(SecureKeyRotation.decide(installedMarker: true, keyExists: false), .create)
        XCTAssertEqual(SecureKeyRotation.decide(installedMarker: false, keyExists: true), .deleteAndCreate)
        XCTAssertEqual(SecureKeyRotation.decide(installedMarker: false, keyExists: false), .create)
    }

    func testStats() {
        let summary = SecureKeyStats.summary((1...100).map(Double.init))
        XCTAssertEqual(summary.p50, 50)
        XCTAssertEqual(summary.p95, 95)
        XCTAssertEqual(summary.max, 100)
        XCTAssertEqual(SecureKeyStats.summary([]).p50, 0)
        XCTAssertEqual(SecureKeyStats.summary([]).p95, 0)
        XCTAssertEqual(SecureKeyStats.summary([]).max, 0)
    }
}
