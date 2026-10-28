import CryptoKit
import Foundation

public enum RecordingKeyWrapError: Error, Equatable {
    case badLength
    case badVersion
    case rejected
}

public enum RecordingKeyWrap {
    public static let version: UInt8 = 1
    public static let aad = Data("rs.recording.v1".utf8)
    public static let keyLength = 32
    public static let wrappedLength = 61

    /// Wraps a 32-byte file key as version || nonce || ciphertext || tag.
    public static func wrap(_ fileKey: Data, master: SymmetricKey) throws -> Data {
        guard fileKey.count == keyLength else {
            throw RecordingKeyWrapError.badLength
        }
        guard master.bitCount == keyLength * 8 else {
            throw RecordingKeyWrapError.rejected
        }

        let nonce = AES.GCM.Nonce()
        let sealed: AES.GCM.SealedBox
        do {
            sealed = try AES.GCM.seal(
                fileKey,
                using: master,
                nonce: nonce,
                authenticating: aad
            )
        } catch {
            throw RecordingKeyWrapError.rejected
        }

        var result = Data(capacity: wrappedLength)
        result.append(version)
        result.append(contentsOf: nonce)
        result.append(sealed.ciphertext)
        result.append(sealed.tag)
        return result
    }

    /// Unwraps version || nonce || ciphertext || tag into a 32-byte file key.
    public static func unwrap(_ blob: Data, master: SymmetricKey) throws -> Data {
        guard blob.count == wrappedLength else {
            throw RecordingKeyWrapError.badLength
        }
        guard blob[blob.startIndex] == version else {
            throw RecordingKeyWrapError.badVersion
        }
        guard master.bitCount == keyLength * 8 else {
            throw RecordingKeyWrapError.rejected
        }

        let nonceData = Data(blob[1..<13])
        let ciphertext = Data(blob[13..<45])
        let tag = Data(blob[45..<wrappedLength])
        do {
            let nonce = try AES.GCM.Nonce(data: nonceData)
            let sealed = try AES.GCM.SealedBox(nonce: nonce, ciphertext: ciphertext, tag: tag)
            var plaintext = try AES.GCM.open(sealed, using: master, authenticating: aad)

            // Data is copy-on-write, so make the returned value independent before
            // clearing the temporary plaintext buffer as a best-effort cleanup.
            let result = Data(plaintext)
            plaintext.resetBytes(in: plaintext.startIndex..<plaintext.endIndex)
            return result
        } catch let error as RecordingKeyWrapError {
            throw error
        } catch {
            throw RecordingKeyWrapError.rejected
        }
    }
}

public enum RecordingWrapCall {
    public static let ok: Int32 = 0
    public static let unavailable: Int32 = -1
    public static let rejected: Int32 = -2
    public static let tooSmall: Int32 = -3

    /// Handles one native wrap/unwrap callback without touching platform APIs.
    public static func handle(
        op: UInt8,
        input: Data,
        capacity: Int,
        master: () -> SymmetricKey?
    ) -> (Int32, Data) {
        switch op {
        case 1 where input.count == RecordingKeyWrap.keyLength,
             2 where input.count == RecordingKeyWrap.wrappedLength:
            break
        default:
            return (rejected, Data())
        }

        guard let master = master() else {
            return (unavailable, Data())
        }

        do {
            let result: Data
            switch op {
            case 1:
                result = try RecordingKeyWrap.wrap(input, master: master)
            case 2:
                result = try RecordingKeyWrap.unwrap(input, master: master)
            default:
                return (rejected, Data())
            }

            guard result.count <= capacity else {
                return (tooSmall, Data())
            }

            return (ok, result)
        } catch {
            return (rejected, Data())
        }
    }
}
