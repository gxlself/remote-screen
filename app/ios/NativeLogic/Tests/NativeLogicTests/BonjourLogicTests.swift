import XCTest
@testable import NativeLogic

final class BonjourLogicTests: XCTestCase {
    func testAddressesPutIPv4FirstDeduplicateAndTruncate() {
        XCTAssertEqual(
            BonjourLogic.orderedAddresses([
                "fe80::1",
                "192.168.1.8",
                "192.168.1.8",
                "10.0.0.2",
                "2001:db8::1",
                "fd00::1",
                "172.16.0.3",
            ]),
            ["192.168.1.8", "10.0.0.2", "172.16.0.3", "fe80::1"]
        )
    }

    func testHostAppendsLocalAndLeavesMissingNameEmpty() {
        XCTAssertEqual(BonjourLogic.host(for: "Mac-mini"), "Mac-mini.local")
        XCTAssertEqual(BonjourLogic.host(for: "Mac-mini.local"), "Mac-mini.local")
        XCTAssertEqual(BonjourLogic.host(for: nil), "")
    }

    func testDuplicateFoundNameEmitsOnce() {
        var state = BonjourDiscoveryState()
        XCTAssertTrue(state.found("Mac-mini"))
        XCTAssertFalse(state.found("Mac-mini"))
    }

    func testUnknownLostNameIsIgnored() {
        var state = BonjourDiscoveryState()
        XCTAssertFalse(state.lost("missing"))
        XCTAssertTrue(state.found("Mac-mini"))
        XCTAssertTrue(state.lost("Mac-mini"))
        XCTAssertFalse(state.lost("Mac-mini"))
    }
}
