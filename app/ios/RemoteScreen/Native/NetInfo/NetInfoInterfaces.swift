import Foundation
import Network

#if canImport(Darwin)
import Darwin
#endif

/// The getifaddrs representation used by NativeLogic's pure classifier.
enum NetInfoInterfaces {
    static func read() -> [NetInfoRawIface] {
        var first: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&first) == 0, let first else { return [] }
        defer { freeifaddrs(first) }

        var grouped: [String: Set<NetInfoRawAddress>] = [:]
        var current: UnsafeMutablePointer<ifaddrs>? = first
        while let item = current {
            let value = item.pointee
            current = value.ifa_next
            let flags = value.ifa_flags
            guard flags & UInt32(IFF_UP) != 0,
                  let address = value.ifa_addr,
                  let netmask = value.ifa_netmask else { continue }

            let family = Int32(address.pointee.sa_family)
            guard family == AF_INET || family == AF_INET6,
                  let ip = addressString(address, family: family),
                  let prefix = prefixLength(netmask, family: family) else { continue }

            let name = String(cString: value.ifa_name)
            grouped[name, default: []].insert(NetInfoRawAddress(ip, prefixLength: prefix))
        }

        return grouped.keys.sorted().map { name in
            NetInfoRawIface(name: name, addrs: grouped[name, default: []].sorted {
                if $0.address == $1.address { return $0.prefixLength < $1.prefixLength }
                return $0.address < $1.address
            })
        }
    }

    private static func addressString(
        _ pointer: UnsafeMutablePointer<sockaddr>,
        family: Int32
    ) -> String? {
        if family == AF_INET {
            let address = pointer.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee.sin_addr }
            var value = address
            var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            guard inet_ntop(AF_INET, &value, &buffer, socklen_t(INET_ADDRSTRLEN)) != nil else { return nil }
            return String(cString: buffer)
        }
        let address = pointer.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) { $0.pointee.sin6_addr }
        var value = address
        var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
        guard inet_ntop(AF_INET6, &value, &buffer, socklen_t(INET6_ADDRSTRLEN)) != nil else { return nil }
        return String(cString: buffer)
    }

    private static func prefixLength(
        _ pointer: UnsafeMutablePointer<sockaddr>,
        family: Int32
    ) -> Int? {
        let bytes: [UInt8]
        if family == AF_INET {
            let mask = pointer.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee.sin_addr }
            let value = UInt32(bigEndian: mask.s_addr)
            bytes = (0..<4).map { index in
                UInt8((value >> UInt32((3 - index) * 8)) & 0xff)
            }
        } else {
            let mask = pointer.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) { $0.pointee.sin6_addr }
            bytes = withUnsafeBytes(of: mask) { Array($0) }
        }
        guard !bytes.isEmpty else { return nil }
        var count = 0
        var sawZero = false
        for byte in bytes {
            if sawZero {
                guard byte == 0 else { return nil }
                continue
            }
            count += byte.nonzeroBitCount
            if byte != 0xff { sawZero = true }
        }
        return count
    }
}
