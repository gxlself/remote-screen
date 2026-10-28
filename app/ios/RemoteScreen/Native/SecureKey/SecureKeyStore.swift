import CryptoKit
import Foundation
import Security

struct SecureKeyInfo {
    let spki: Data
    let secureEnclave: Bool
    let rotatedThisLaunch: Bool
}

enum SecureKeyStoreError: Error, LocalizedError {
    case keychain(OSStatus, String)
    case coreFoundation(String)
    case invalidKey(String)

    var errorDescription: String? {
        switch self {
        case let .keychain(status, operation):
            return "\(operation) failed (OSStatus \(status))"
        case let .coreFoundation(description):
            return description
        case let .invalidKey(description):
            return description
        }
    }
}

final class SecureKeyStore {
    static let shared = SecureKeyStore()

    private static let installedMarker = "rs.secureKey.installed.v1"
    private static let sampleLimit = 256

    private let queue = DispatchQueue(label: "ai.gxlself.remotescreen.secure-key")
    private var key: SecKey?
    private var cachedInfo: SecureKeyInfo?
    private var signSamples = [Double](repeating: 0, count: sampleLimit)
    private var signSampleCount = 0
    private var signSampleIndex = 0

    private init() {}

    func loadOrCreate() throws -> SecureKeyInfo {
        try queue.sync {
            try loadOrCreateLocked()
        }
    }

    func sign(_ msg: Data) throws -> Data {
        try signWithDuration(msg).signature
    }

    func signWithDuration(_ msg: Data) throws -> (signature: Data, durationMs: Double) {
        try queue.sync {
            let started = DispatchTime.now().uptimeNanoseconds
            defer {
                let elapsed = DispatchTime.now().uptimeNanoseconds &- started
                recordSample(Double(elapsed) / 1_000_000)
            }
            let signingKey: SecKey
            if let key {
                signingKey = key
            } else {
                _ = try loadOrCreateLocked()
                guard let loadedKey = key else {
                    throw SecureKeyStoreError.invalidKey("device key is unavailable")
                }
                signingKey = loadedKey
            }

            var cfError: Unmanaged<CFError>?
            guard let signature = SecKeyCreateSignature(
                signingKey,
                .ecdsaSignatureMessageX962SHA256,
                msg as CFData,
                &cfError
            ) else {
                throw makeCoreFoundationError(cfError, operation: "SecKeyCreateSignature")
            }
            let elapsed = DispatchTime.now().uptimeNanoseconds &- started
            return (signature as Data, Double(elapsed) / 1_000_000)
        }
    }

    func recentSignSamplesMs() -> [Double] {
        queue.sync {
            guard signSampleCount > 0 else { return [] }
            if signSampleCount < Self.sampleLimit {
                return Array(signSamples.prefix(signSampleCount))
            }
            return Array(signSamples[signSampleIndex...]) + Array(signSamples[..<signSampleIndex])
        }
    }

    private var keychainLabel: String {
        let bundleIdentifier = Bundle.main.bundleIdentifier ?? "ai.gxlself.remotescreen"
        return "\(bundleIdentifier).device-key.v1"
    }

    private func loadOrCreateLocked() throws -> SecureKeyInfo {
        if let cachedInfo, key != nil {
            return cachedInfo
        }

        let installed = UserDefaults.standard.bool(forKey: Self.installedMarker)
        let existing = try existingKeyLocked()
        let action = SecureKeyRotation.decide(installedMarker: installed, keyExists: existing != nil)

        let signingKey: SecKey
        let rotated: Bool
        switch action {
        case .useExisting:
            guard let existing else {
                throw SecureKeyStoreError.invalidKey("keychain key disappeared")
            }
            signingKey = existing
            rotated = false
        case .create:
            signingKey = try createKeyLocked()
            rotated = true
        case .deleteAndCreate:
            try deleteKeyLocked()
            signingKey = try createKeyLocked()
            rotated = true
        }

        let spki = try spkiLocked(for: signingKey)
        let secureEnclave = isSecureEnclaveKey(signingKey)
        key = signingKey
        let info = SecureKeyInfo(
            spki: spki,
            secureEnclave: secureEnclave,
            rotatedThisLaunch: rotated
        )
        cachedInfo = info
        UserDefaults.standard.set(true, forKey: Self.installedMarker)
        if rotated && !secureEnclave {
            RsLog.marker("SECUREKEY", ["warn": "no-secure-enclave"])
        }
        return info
    }

    private func existingKeyLocked() throws -> SecKey? {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(keyQuery(returning: true) as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            guard let result else {
                throw SecureKeyStoreError.invalidKey("keychain item is not a SecKey")
            }
            guard CFGetTypeID(result) == SecKeyGetTypeID() else {
                throw SecureKeyStoreError.invalidKey("keychain item is not a SecKey")
            }
            return (result as! SecKey)
        case errSecItemNotFound:
            return nil
        default:
            throw SecureKeyStoreError.keychain(status, "SecItemCopyMatching")
        }
    }

    private func createKeyLocked() throws -> SecKey {
        let useSecureEnclave = CryptoKit.SecureEnclave.isAvailable
        var accessError: Unmanaged<CFError>?
        let flags: SecAccessControlCreateFlags = useSecureEnclave ? [.privateKeyUsage] : []
        guard let access = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            flags,
            &accessError
        ) else {
            throw makeCoreFoundationError(accessError, operation: "SecAccessControlCreateWithFlags")
        }

        var attributes: [String: Any] = [
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits as String: 256,
            kSecPrivateKeyAttrs as String: [
                kSecAttrIsPermanent as String: true,
                kSecAttrLabel as String: keychainLabel,
                kSecAttrAccessControl as String: access,
            ],
        ]
        if useSecureEnclave {
            attributes[kSecAttrTokenID as String] = kSecAttrTokenIDSecureEnclave
        }

        var cfError: Unmanaged<CFError>?
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &cfError) else {
            throw makeCoreFoundationError(cfError, operation: "SecKeyCreateRandomKey")
        }
        return key
    }

    private func deleteKeyLocked() throws {
        let status = SecItemDelete(keyQuery(returning: false) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SecureKeyStoreError.keychain(status, "SecItemDelete")
        }
    }

    private func keyQuery(returning: Bool) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassKey,
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrLabel as String: keychainLabel,
        ]
        if returning {
            query[kSecReturnRef as String] = true
        }
        return query
    }

    private func spkiLocked(for key: SecKey) throws -> Data {
        guard let publicKey = SecKeyCopyPublicKey(key) else {
            throw SecureKeyStoreError.invalidKey("SecKeyCopyPublicKey returned nil")
        }
        var cfError: Unmanaged<CFError>?
        guard let external = SecKeyCopyExternalRepresentation(publicKey, &cfError) else {
            throw makeCoreFoundationError(cfError, operation: "SecKeyCopyExternalRepresentation")
        }
        do {
            return try SecureKeySpki.der(x963: external as Data)
        } catch {
            throw SecureKeyStoreError.invalidKey("public key is not an uncompressed P-256 key")
        }
    }

    private func isSecureEnclaveKey(_ key: SecKey) -> Bool {
        guard let attributes = SecKeyCopyAttributes(key) as NSDictionary? else { return false }
        return (attributes[kSecAttrTokenID] as? String) == (kSecAttrTokenIDSecureEnclave as String)
    }

    private func recordSample(_ sampleMs: Double) {
        signSamples[signSampleIndex] = sampleMs
        signSampleIndex = (signSampleIndex + 1) % Self.sampleLimit
        signSampleCount = min(signSampleCount + 1, Self.sampleLimit)
    }

    private func makeCoreFoundationError(
        _ error: Unmanaged<CFError>?,
        operation: String
    ) -> SecureKeyStoreError {
        guard let error else {
            return .coreFoundation("\(operation) failed without a CFError")
        }
        let retained = error.takeRetainedValue()
        return .coreFoundation("\(operation) failed: \((retained as Error).localizedDescription)")
    }
}
