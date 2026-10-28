import CryptoKit
import Foundation
import Security

final class RecordingKeyStore {
    static let shared = RecordingKeyStore()

    private static let service = "ai.gxlself.remotescreen.recording"
    private static let account = "master-v1"
    private static let keyLength = 32

    private let lock = NSLock()
    private var cachedMaster: SymmetricKey?

    private init() {}

    func master() -> SymmetricKey? {
        lock.lock()
        defer { lock.unlock() }

        if let cachedMaster {
            return cachedMaster
        }

        switch readLocked() {
        case let .success(data):
            return cacheLocked(data)
        case .notFound:
            return createLocked()
        case .unavailable, .failure:
            return nil
        }
    }

    private enum ReadResult {
        case success(Data)
        case notFound
        case unavailable
        case failure
    }

    private func readLocked() -> ReadResult {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query() as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            guard let data = result as? Data, data.count == Self.keyLength else {
                RsLog.marker("RECKEY", ["error": "badLength"])
                return .failure
            }
            return .success(data)
        case errSecItemNotFound:
            return .notFound
        case errSecInteractionNotAllowed:
            return .unavailable
        default:
            return .failure
        }
    }

    private func createLocked() -> SymmetricKey? {
        var data = Data(repeating: 0, count: Self.keyLength)
        let randomStatus = data.withUnsafeMutableBytes { buffer -> OSStatus in
            guard let baseAddress = buffer.baseAddress else {
                return errSecParam
            }
            return SecRandomCopyBytes(kSecRandomDefault, Self.keyLength, baseAddress)
        }
        guard randomStatus == errSecSuccess else {
            return nil
        }

        let attributes: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecAttrSynchronizable as String: false,
            kSecValueData as String: data,
        ]
        let status = SecItemAdd(attributes as CFDictionary, nil)
        switch status {
        case errSecSuccess:
            return cacheLocked(data)
        case errSecDuplicateItem:
            // Another process may have created the item after our read. Read it
            // once and use that value; never replace an existing item.
            guard case let .success(existing) = readLocked() else {
                return nil
            }
            return cacheLocked(existing)
        default:
            return nil
        }
    }

    private func cacheLocked(_ data: Data) -> SymmetricKey {
        let key = SymmetricKey(data: data)
        cachedMaster = key
        return key
    }

    private func query() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
            kSecAttrSynchronizable as String: false,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
    }
}
