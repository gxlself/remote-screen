import CryptoKit
import Foundation

enum SecureKeySpki {
    private static let p256SpkiPrefix = Data([
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2A, 0x86, 0x48,
        0xCE, 0x3D, 0x02, 0x01, 0x06, 0x08, 0x2A, 0x86, 0x48,
        0xCE, 0x3D, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ])

    static func der(x963: Data) throws -> Data {
        guard x963.count == 65, x963.first == 0x04 else {
            throw SecureKeySpkiError.invalidX963
        }
        return p256SpkiPrefix + x963
    }

    static func fingerprint(_ spki: Data) -> Data {
        Data(SHA256.hash(data: spki))
    }

    static func formatFingerprint(_ fp: Data) -> String {
        let bytes = fp.map { String(format: "%02X", $0) }
        return "SHA256 " + bytes.joined(separator: ":")
    }

    static func deviceId(_ fp: Data) -> String {
        let bytes = Array(fp.prefix(10))
        guard !bytes.isEmpty else { return "" }

        let alphabet = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
        var output = ""
        output.reserveCapacity((bytes.count * 8 + 4) / 5)
        var buffer = 0
        var bits = 0
        for byte in bytes {
            for shift in stride(from: 7, through: 0, by: -1) {
                buffer = (buffer << 1) | Int((byte >> UInt8(shift)) & 1)
                bits += 1
                if bits == 5 {
                    output.append(alphabet[buffer & 0x1F])
                    buffer = 0
                    bits = 0
                }
            }
        }
        if bits > 0 {
            output.append(alphabet[(buffer << (5 - bits)) & 0x1F])
        }
        return output.lowercased()
    }
}

enum SecureKeySpkiError: Error, Equatable {
    case invalidX963
}
