import Foundation

/// Pure state and value helpers used by the iOS Bonjour browser.
///
/// Keeping these transformations outside Network.framework makes the parts
/// that affect the JavaScript contract straightforward to test.
enum BonjourLogic {
    static func host(for serviceName: String?) -> String {
        guard var value = serviceName?.trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty else {
            return ""
        }
        if value.lowercased().hasSuffix(".local") {
            return value
        }
        value.append(contentsOf: ".local")
        return value
    }

    /// Removes duplicate addresses, puts IPv4 addresses first, and limits the
    /// result to the number exposed by the Bonjour contract.
    static func orderedAddresses(_ addresses: [String], limit: Int = 4) -> [String] {
        guard limit > 0 else { return [] }

        var seen = Set<String>()
        var ipv4: [String] = []
        var other: [String] = []
        for address in addresses {
            guard !address.isEmpty, seen.insert(address).inserted else { continue }
            if isIPv4(address) {
                ipv4.append(address)
            } else {
                other.append(address)
            }
        }
        return Array((ipv4 + other).prefix(limit))
    }

    private static func isIPv4(_ address: String) -> Bool {
        let parts = address.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return false }
        return parts.allSatisfy { part in
            guard !part.isEmpty,
                  part.allSatisfy({ $0.isNumber }),
                  let value = Int(part) else {
                return false
            }
            return (0...255).contains(value)
        }
    }
}

/// Tracks names currently represented by browser results. A Bonjour browser
/// can report multiple interfaces for the same service; the JS side should see
/// one found event and one lost event for that name.
struct BonjourDiscoveryState {
    private(set) var names: Set<String> = []

    @discardableResult
    mutating func found(_ name: String) -> Bool {
        guard !name.isEmpty else { return false }
        return names.insert(name).inserted
    }

    @discardableResult
    mutating func lost(_ name: String) -> Bool {
        names.remove(name) != nil
    }

    mutating func reset() {
        names.removeAll(keepingCapacity: false)
    }
}
