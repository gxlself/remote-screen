import XCTest
@testable import NativeLogic

final class SystemTests: XCTestCase {
    func testProbeStateTerminalResults() {
        var state = LocalNetworkProbeState()
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.foundSelf),
            "granted"
        )

        state = LocalNetworkProbeState()
        // NWError only conforms to CustomNSError on iOS 16.4+. On iOS
        // 15.1–16.3, forward NWError.dns's code directly: NSError bridging
        // does not preserve the -65570 PolicyDenied code.
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.waiting(-65570)),
            "denied"
        )

        state = LocalNetworkProbeState()
        XCTAssertNil(
            state.receive(LocalNetworkProbeState.Event.waiting(-65563))
        )
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.timeout),
            "unknown"
        )

        state = LocalNetworkProbeState()
        XCTAssertNil(
            state.receive(LocalNetworkProbeState.Event.foundOther)
        )
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.foundSelf),
            "granted"
        )

        state = LocalNetworkProbeState()
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.timeout),
            "unknown"
        )
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.foundSelf),
            "unknown"
        )

        state = LocalNetworkProbeState()
        XCTAssertEqual(
            state.receive(LocalNetworkProbeState.Event.listenerFailed),
            "unknown"
        )
    }

    func testHapticKindParsing() {
        XCTAssertEqual(SystemHapticKind.parse("success"), .success)
        XCTAssertNil(SystemHapticKind.parse("boom"))
    }
}
