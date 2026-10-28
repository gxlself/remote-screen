import Foundation
import Network

struct NetInfoRawAddress: Equatable, Hashable {
    let address: String
    let prefixLength: Int

    init(_ address: String, prefixLength: Int) {
        self.address = address
        self.prefixLength = prefixLength
    }
}

struct NetInfoRawIface: Equatable, Hashable {
    let name: String
    let addrs: [NetInfoRawAddress]

    init(name: String, addrs: [NetInfoRawAddress]) {
        self.name = name
        self.addrs = addrs
    }
}

struct NetInfoIface: Equatable, Hashable {
    let name: String
    let addrs: [String]
    let isVpn: Bool
}

enum NetInfoClassifier {
    static func isLinkLocal(_ address: String) -> Bool {
        guard let cidr = NetInfoCidr.parse(address) else { return false }
        return NetInfoCidr.parse("169.254.0.0/16")?.contains(cidr) == true
            || NetInfoCidr.parse("fe80::/10")?.contains(cidr) == true
    }

    static func isLoopback(_ address: String) -> Bool {
        guard let cidr = NetInfoCidr.parse(address) else { return false }
        return NetInfoCidr.parse("127.0.0.0/8")?.contains(cidr) == true
            || NetInfoCidr.parse("::1/128")?.contains(cidr) == true
    }

    static func isTailnet(_ address: String) -> Bool {
        guard let cidr = NetInfoCidr.parse(address) else { return false }
        return NetInfoCidr.parse("100.64.0.0/10")?.contains(cidr) == true
            || NetInfoCidr.parse("fd7a:115c:a1e0::/48")?.contains(cidr) == true
    }

    static func isVpn(_ iface: NetInfoRawIface, pools: [NetInfoCidr]) -> Bool {
        if iface.addrs.contains(where: { address in
            pools.contains { $0.contains(address.address) }
        }) {
            return true
        }
        let prefixes = ["utun", "ipsec", "ppp", "tun", "tap", "wg"]
        guard prefixes.contains(where: { iface.name.hasPrefix($0) }) else { return false }
        // A Tailscale utun carries only tailnet addresses and is deliberately
        // excluded. This mirrors rs-mobile::rscore::netinfo::is_vpn exactly.
        return !iface.addrs.allSatisfy { isTailnet($0.address) }
    }

    static func snapshot(_ raw: [NetInfoRawIface], pools: [NetInfoCidr]) -> NetInfoSnapshot {
        let ifaces = raw.compactMap { rawIface -> NetInfoIface? in
            let usable = rawIface.addrs.filter { rawAddress in
                !isLinkLocal(rawAddress.address) && !isLoopback(rawAddress.address)
            }
            let addresses = usable.compactMap { rawAddress -> String? in
                guard !isLinkLocal(rawAddress.address), !isLoopback(rawAddress.address),
                      let parsed = NetInfoCidr.parse(rawAddress.address) else { return nil }
                return format(NetInfoCidr(family: parsed.family, bytes: parsed.bytes, prefixLength: rawAddress.prefixLength))
            }
            var uniqueAddresses: [String] = []
            uniqueAddresses.reserveCapacity(addresses.count)
            for address in addresses where !uniqueAddresses.contains(address) {
                uniqueAddresses.append(address)
            }
            guard !uniqueAddresses.isEmpty else { return nil }
            let filteredIface = NetInfoRawIface(name: rawIface.name, addrs: usable)
            return NetInfoIface(
                name: rawIface.name,
                addrs: uniqueAddresses,
                isVpn: isVpn(filteredIface, pools: pools)
            )
        }
        .sorted { $0.name < $1.name }
        return NetInfoSnapshot(ifaces: ifaces)
    }

    private static func format(_ cidr: NetInfoCidr) -> String {
        let address: String
        switch cidr.family {
        case .v4:
            address = cidr.bytes.map(String.init).joined(separator: ".")
        case .v6:
            var words: [String] = []
            for index in stride(from: 0, to: cidr.bytes.count, by: 2) {
                words.append(String(UInt16(cidr.bytes[index]) << 8 | UInt16(cidr.bytes[index + 1]), radix: 16))
            }
            address = compressIPv6(words)
        }
        return "\(address)/\(cidr.prefixLength)"
    }

    private static func compressIPv6(_ words: [String]) -> String {
        let values = words.map { Int($0, radix: 16) ?? 0 }
        var bestStart = -1
        var bestLength = 0
        var start = 0
        while start < values.count {
            guard values[start] == 0 else { start += 1; continue }
            var end = start
            while end < values.count, values[end] == 0 { end += 1 }
            if end - start > bestLength {
                bestStart = start
                bestLength = end - start
            }
            start = end
        }
        guard bestLength >= 2 else { return words.joined(separator: ":") }
        let left = words[..<bestStart].joined(separator: ":")
        let right = words[(bestStart + bestLength)...].joined(separator: ":")
        if left.isEmpty, right.isEmpty { return "::" }
        if left.isEmpty { return "::\(right)" }
        if right.isEmpty { return "\(left)::" }
        return "\(left)::\(right)"
    }
}
