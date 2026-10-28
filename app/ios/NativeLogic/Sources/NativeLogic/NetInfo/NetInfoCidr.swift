import Foundation
import Network

/// A parsed IPv4 or IPv6 network.  The parser intentionally accepts a single
/// address without a prefix and treats it as a host network (/32 or /128).
struct NetInfoCidr: Equatable, Hashable {
    enum Family: Equatable, Hashable {
        case v4
        case v6
    }

    let family: Family
    let bytes: [UInt8]
    let prefixLength: Int

    static func parse(_ value: String) -> NetInfoCidr? {
        let pieces = value.trimmingCharacters(in: .whitespacesAndNewlines).split(
            separator: "/",
            omittingEmptySubsequences: false
        )
        guard pieces.count == 1 || pieces.count == 2 else { return nil }
        let address = String(pieces[0])
        guard !address.isEmpty else { return nil }

        if let ipv4 = IPv4Address(address) {
            let prefix = pieces.count == 2 ? Int(pieces[1]) : 32
            guard let prefix, (0...32).contains(prefix) else { return nil }
            return NetInfoCidr(family: .v4, bytes: Array(ipv4.rawValue), prefixLength: prefix)
        }
        if let ipv6 = IPv6Address(address) {
            let prefix = pieces.count == 2 ? Int(pieces[1]) : 128
            guard let prefix, (0...128).contains(prefix) else { return nil }
            return NetInfoCidr(family: .v6, bytes: Array(ipv6.rawValue), prefixLength: prefix)
        }
        return nil
    }

    func contains(_ address: String) -> Bool {
        guard let other = NetInfoCidr.parse(address), other.prefixLength <= (family == .v4 ? 32 : 128) else {
            return false
        }
        return contains(other.bytes, family: other.family)
    }

    func contains(_ address: NetInfoCidr) -> Bool {
        contains(address.bytes, family: address.family)
    }

    private func contains(_ otherBytes: [UInt8], family otherFamily: Family) -> Bool {
        guard family == otherFamily else { return false }
        let wholeBytes = prefixLength / 8
        let remainingBits = prefixLength % 8
        guard bytes.count == otherBytes.count, bytes.prefix(wholeBytes) == otherBytes.prefix(wholeBytes) else {
            return false
        }
        guard remainingBits > 0 else { return true }
        let mask = UInt8((0xff << (8 - remainingBits)) & 0xff)
        return bytes[wholeBytes] & mask == otherBytes[wholeBytes] & mask
    }
}
