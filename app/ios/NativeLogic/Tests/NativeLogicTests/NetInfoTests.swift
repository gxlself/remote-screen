import XCTest
@testable import NativeLogic

final class NetInfoTests: XCTestCase {
    private func iface(_ name: String, _ addresses: String...) -> NetInfoRawIface {
        NetInfoRawIface(
            name: name,
            addrs: addresses.map { value in
                let pieces = value.split(separator: "/", maxSplits: 1)
                return NetInfoRawAddress(
                    String(pieces[0]),
                    prefixLength: pieces.count == 2 ? Int(pieces[1])! : 32
                )
            }
        )
    }

    func testClassifierMatchesA16HVectors() {
        let pools = [NetInfoCidr.parse("10.99.0.0/16")!]
        let status = NetInfoClassifier.snapshot([
            iface("en0", "192.168.31.20/24", "fe80::1/64"),
            iface("lo0", "127.0.0.1/8"),
            iface("utun4", "100.64.0.21/32"),
            iface("utun5", "10.99.4.27/32"),
            iface("ipsec0", "10.30.8.9/32"),
            iface("en2", "10.99.4.27/24"),
            iface("utun0", "fe80::2/64"),
        ], pools: pools)

        XCTAssertEqual(status.ifaces.map(\.name), ["en0", "en2", "ipsec0", "utun4", "utun5"])
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "en0" })?.addrs, ["192.168.31.20/24"])
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "en0" })?.isVpn, false)
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "utun4" })?.isVpn, false)
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "utun5" })?.isVpn, true)
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "ipsec0" })?.isVpn, true)
        XCTAssertEqual(status.ifaces.first(where: { $0.name == "en2" })?.isVpn, true)

        let withoutPools = NetInfoClassifier.snapshot([iface("utun5", "10.99.4.27/32")], pools: [])
        XCTAssertTrue(withoutPools.ifaces[0].isVpn)
    }

    func testSnapshotEqualityAndOrdering() {
        let raw = [iface("z", "10.30.1.2/16"), iface("a", "192.168.31.20/24")]
        let first = NetInfoClassifier.snapshot(raw, pools: [])
        let second = NetInfoClassifier.snapshot(raw, pools: [])
        XCTAssertEqual(first, second)
        XCTAssertNotEqual(first, NetInfoClassifier.snapshot(raw + [iface("a", "10.30.1.2/16")], pools: []))
        XCTAssertEqual(first.ifaces.map(\.name), ["a", "z"])
    }

    func testCidrParsingAndContains() {
        let v4 = NetInfoCidr.parse("10.99.0.0/16")
        XCTAssertTrue(v4?.contains("10.99.255.1") == true)
        XCTAssertFalse(v4?.contains("10.98.0.1") == true)
        XCTAssertTrue(NetInfoCidr.parse("fd7a:115c:a1e0::/48")?.contains("fd7a:115c:a1e0:ab12::1") == true)
        XCTAssertNil(NetInfoCidr.parse("10.99.0.0/nope"))
        XCTAssertNil(NetInfoCidr.parse("not-an-address"))
    }

    func testDebouncerUsesTrailingEdge() {
        var debouncer = NetInfoDebouncer(delay: 500)
        var fires = 0
        debouncer.update(at: 0)
        debouncer.update(at: 100)
        debouncer.update(at: 300)
        debouncer.fireIfDue(at: 799) { fires += 1 }
        XCTAssertEqual(fires, 0)
        debouncer.fireIfDue(at: 800) { fires += 1 }
        XCTAssertEqual(fires, 1)
        debouncer.fireIfDue(at: 900) { fires += 1 }
        XCTAssertEqual(fires, 1)
    }
}
