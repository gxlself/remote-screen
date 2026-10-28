import CryptoKit
import Foundation
import Security

private final class RsDtSeHandle {
    let key: SecureEnclave.P256.Signing.PrivateKey

    init(_ key: SecureEnclave.P256.Signing.PrivateKey) {
        self.key = key
    }
}

private func report(
    _ error: Error,
    _ output: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) {
    let nsError = error as NSError
    writeError(
        "\(String(describing: error)) (domain \(nsError.domain), code \(nsError.code))",
        output,
        capacity
    )
}

private func report(
    _ message: String,
    _ output: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) {
    writeError(message, output, capacity)
}

private func cString(_ pointer: UnsafePointer<CChar>?) -> String? {
    guard let pointer else { return nil }
    return String(cString: pointer)
}

private func keychainQuery(service: String, account: String, returningData: Bool) -> [String: Any] {
    var query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: account,
    ]
    if returningData {
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
    }
    return query
}

@_cdecl("rs_dt_se_available")
public func rs_dt_se_available() -> Bool {
    SecureEnclave.isAvailable
}

@_cdecl("rs_dt_se_create")
public func rs_dt_se_create(
    _ blob: UnsafeMutableRawPointer?,
    _ x963: UnsafeMutableRawPointer?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int32 {
    guard let blob, let x963 else {
        report("output buffer is null", errorBuffer, capacity)
        return -1
    }
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey()
        storeBuf(blob, key.dataRepresentation)
        storeBuf(x963, key.publicKey.x963Representation)
        return 0
    } catch {
        report(error, errorBuffer, capacity)
        return -1
    }
}

@_cdecl("rs_dt_se_open")
public func rs_dt_se_open(
    _ blob: UnsafePointer<UInt8>?,
    _ length: Int,
    _ x963: UnsafeMutableRawPointer?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> UnsafeMutableRawPointer? {
    guard let blob, length >= 0, let x963 else {
        report("invalid key blob arguments", errorBuffer, capacity)
        return nil
    }
    do {
        let data = Data(bytes: blob, count: length)
        let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: data)
        storeBuf(x963, key.publicKey.x963Representation)
        return Unmanaged.passRetained(RsDtSeHandle(key)).toOpaque()
    } catch {
        report(error, errorBuffer, capacity)
        return nil
    }
}

@_cdecl("rs_dt_se_sign")
public func rs_dt_se_sign(
    _ handle: UnsafeMutableRawPointer?,
    _ message: UnsafePointer<UInt8>?,
    _ length: Int,
    _ signature: UnsafeMutableRawPointer?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int32 {
    guard let handle, let signature, let message, length >= 0 else {
        report("invalid signing arguments", errorBuffer, capacity)
        return -1
    }
    do {
        let owner = Unmanaged<RsDtSeHandle>.fromOpaque(handle).takeUnretainedValue()
        let data = Data(bytes: message, count: length)
        storeBuf(signature, try owner.key.signature(for: data).derRepresentation)
        return 0
    } catch {
        report(error, errorBuffer, capacity)
        return -1
    }
}

@_cdecl("rs_dt_se_close")
public func rs_dt_se_close(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<RsDtSeHandle>.fromOpaque(handle).release()
}

@_cdecl("rs_dt_kc_get")
public func rs_dt_kc_get(
    _ servicePointer: UnsafePointer<CChar>?,
    _ accountPointer: UnsafePointer<CChar>?,
    _ output: UnsafeMutableRawPointer?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int32 {
    guard let service = cString(servicePointer),
          let account = cString(accountPointer),
          let output
    else {
        report("invalid keychain arguments", errorBuffer, capacity)
        return -1
    }
    var item: CFTypeRef?
    let status = SecItemCopyMatching(
        keychainQuery(service: service, account: account, returningData: true) as CFDictionary,
        &item
    )
    switch status {
    case errSecSuccess:
        let data: Data
        if let returned = item as? Data {
            data = returned
        } else if let returned = item as? NSData {
            data = returned as Data
        } else {
            report("keychain item is not data", errorBuffer, capacity)
            return -1
        }
        storeBuf(output, data)
        return 0
    case errSecItemNotFound:
        return 1
    default:
        report("SecItemCopyMatching failed (OSStatus \(status))", errorBuffer, capacity)
        return -1
    }
}

@_cdecl("rs_dt_kc_set")
public func rs_dt_kc_set(
    _ servicePointer: UnsafePointer<CChar>?,
    _ accountPointer: UnsafePointer<CChar>?,
    _ value: UnsafePointer<UInt8>?,
    _ length: Int,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int32 {
    guard let service = cString(servicePointer),
          let account = cString(accountPointer),
          length >= 0,
          (length == 0 || value != nil)
    else {
        report("invalid keychain arguments", errorBuffer, capacity)
        return -1
    }
    let query = keychainQuery(service: service, account: account, returningData: false)
    let deleteStatus = SecItemDelete(query as CFDictionary)
    guard deleteStatus == errSecSuccess || deleteStatus == errSecItemNotFound else {
        report("SecItemDelete failed (OSStatus \(deleteStatus))", errorBuffer, capacity)
        return -1
    }
    let data = value.map { Data(bytes: $0, count: length) } ?? Data()
    let item: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: account,
        kSecValueData as String: data,
        kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    let status = SecItemAdd(item as CFDictionary, nil)
    guard status == errSecSuccess else {
        report("SecItemAdd failed (OSStatus \(status))", errorBuffer, capacity)
        return -1
    }
    return 0
}

@_cdecl("rs_dt_kc_delete")
public func rs_dt_kc_delete(
    _ servicePointer: UnsafePointer<CChar>?,
    _ accountPointer: UnsafePointer<CChar>?
) -> Int32 {
    guard let service = cString(servicePointer), let account = cString(accountPointer) else {
        return -1
    }
    let status = SecItemDelete(keychainQuery(service: service, account: account, returningData: false) as CFDictionary)
    return status == errSecSuccess || status == errSecItemNotFound ? 0 : -1
}
