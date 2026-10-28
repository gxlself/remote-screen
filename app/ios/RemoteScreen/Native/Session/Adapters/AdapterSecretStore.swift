import Foundation
import Security

enum AdapterSecretStore {
    // Swift initializes static lets once, including concurrent launch callers.
    private static let registration: Void = {
        let defaults = UserDefaults.standard
        var purged = false
        if AdapterKeychainLogic.shouldPurge(
            installedFlag: defaults.bool(forKey: AdapterKeychainLogic.installedFlagKey)
        ) {
            // Purge by service only: no account or synchronizable filter.
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: AdapterKeychainLogic.service,
            ]
            let status = SecItemDelete(query as CFDictionary)
            _ = AdapterSecretStore.result(operation: "purge", code: status)
            if AdapterKeychainLogic.deleteCode(status: status) == 0 {
                defaults.set(true, forKey: AdapterKeychainLogic.installedFlagKey)
                purged = true
            }
        }

        // Register even if a locked device prevented the install-time purge.
        rs_register_secret_store(
            nil, AdapterSecretStore.getCallback, AdapterSecretStore.putCallback, AdapterSecretStore.delCallback
        )
        RsLog.marker("ADAPTER-SECRETS", ["registered": true, "purged": purged])
    }()

    static func registerAtLaunch() {
        _ = registration
    }

    // These capture-free callbacks and their nil context live for the process.
    // SecItem APIs support concurrent calls; never wait for the main thread.
    private static let getCallback: @convention(c) (
        UnsafeMutableRawPointer?, UnsafePointer<CChar>?,
        UnsafeMutablePointer<UInt8>?, UnsafeMutablePointer<Int>?
    ) -> Int32 = { _, key, buffer, length in
        guard let account = AdapterSecretStore.account(from: key), let buffer, let length else {
            return AdapterSecretStore.result(operation: "get", code: -1)
        }
        let capacity = length.pointee
        var query = AdapterSecretStore.query(for: account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        let code = AdapterKeychainLogic.getCode(status: status)
        guard code == 0 else { return AdapterSecretStore.result(operation: "get", code: code) }
        guard let data = item as? Data,
              AdapterKeychainLogic.fits(valueCount: data.count, capacity: capacity) else {
            return AdapterSecretStore.result(operation: "get", code: -1)
        }

        var bytes = [UInt8](data)
        defer {
            bytes.withUnsafeMutableBytes { storage in
                if let address = storage.baseAddress {
                    _ = memset(address, 0, storage.count)
                }
            }
        }
        bytes.withUnsafeBufferPointer { storage in
            if let address = storage.baseAddress {
                buffer.update(from: address, count: storage.count)
            }
        }
        // On failure the caller's capacity and buffer are left untouched.
        length.pointee = bytes.count
        return AdapterSecretStore.result(operation: "get", code: 0)
    }

    private static let putCallback: @convention(c) (
        UnsafeMutableRawPointer?, UnsafePointer<CChar>?, UnsafePointer<UInt8>?, Int
    ) -> Int32 = { _, key, value, length in
        guard let account = AdapterSecretStore.account(from: key), let value,
              length > 0, length <= AdapterKeychainLogic.maxValueBytes else {
            return AdapterSecretStore.result(operation: "put", code: -1)
        }

        let query = AdapterSecretStore.query(for: account)
        let attributes: [String: Any] = [
            kSecValueData as String: Data(bytes: value, count: length),
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        var step = AdapterKeychainLogic.PutStep.update
        while true {
            let status: OSStatus
            switch step {
            case .update, .updateAgain:
                status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
            case .add:
                let item = query.merging(attributes) { _, attribute in attribute }
                status = SecItemAdd(item as CFDictionary, nil)
            }
            guard let next = AdapterKeychainLogic.nextPutStep(after: step, status: status) else {
                return AdapterSecretStore.result(
                    operation: "put", code: AdapterKeychainLogic.putCode(lastStatus: status)
                )
            }
            step = next
        }
    }

    private static let delCallback: @convention(c) (
        UnsafeMutableRawPointer?, UnsafePointer<CChar>?
    ) -> Int32 = { _, key in
        guard let account = AdapterSecretStore.account(from: key) else {
            return AdapterSecretStore.result(operation: "del", code: -1)
        }
        let status = SecItemDelete(AdapterSecretStore.query(for: account) as CFDictionary)
        return AdapterSecretStore.result(operation: "del", code: AdapterKeychainLogic.deleteCode(status: status))
    }

    private static func account(from key: UnsafePointer<CChar>?) -> String? {
        guard let key else { return nil }
        var bytes: [UInt8] = []
        // Inspect at most maxKeyBytes + 1 bytes to reject oversized C strings.
        for index in 0...AdapterKeychainLogic.maxKeyBytes {
            let byte = UInt8(bitPattern: key[index])
            if byte == 0 { return AdapterKeychainLogic.account(fromKeyBytes: bytes) }
            bytes.append(byte)
        }
        return nil
    }

    private static func query(for account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: AdapterKeychainLogic.service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
        ]
    }

    private static func result(operation: String, code: Int32) -> Int32 {
        RsLog.marker("ADAPTER-SECRETS", ["operation": operation, "status": code])
        return code
    }
}
