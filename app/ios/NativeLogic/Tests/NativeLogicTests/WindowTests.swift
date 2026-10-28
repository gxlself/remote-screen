import Foundation
import XCTest
@testable import NativeLogic

final class WindowTests: XCTestCase {
    private let request = WindowRequest(
        windowId: "wlx3k2p1",
        payload: #"{"kind":"host","sessionId":"s1","hostId":"h1","name":"Mac"}"#,
        title: "Mac"
    )

    func testActivityTypeUsesTheBundleIdPrefix() {
        // Info.plist NSUserActivityTypes lists the same string.
        XCTAssertEqual(WindowActivity.type, "ai.gxlself.remotescreen.window")
    }

    func testUserInfoRoundTrips() {
        let info: [AnyHashable: Any] = WindowActivity.userInfo(request)
        XCTAssertEqual(WindowActivity.decode(info), request)
    }

    func testSessionInfoDecodesLikeTheActivity() {
        let info: [String: Any] = WindowActivity.userInfo(request)
        XCTAssertEqual(WindowActivity.decode(sessionInfo: info), request)
        XCTAssertNil(WindowActivity.decode(sessionInfo: nil))
        XCTAssertNil(WindowActivity.decode(sessionInfo: ["windowId": "main", "payload": "{}"]))
    }

    func testDecodeRejectsMainAndMissingOrBadFields() {
        XCTAssertNil(WindowActivity.decode(nil))
        XCTAssertNil(WindowActivity.decode([:]))
        XCTAssertNil(WindowActivity.decode(["windowId": "w1"]))
        XCTAssertNil(WindowActivity.decode(["payload": "{}"]))
        XCTAssertNil(WindowActivity.decode(["windowId": "main", "payload": "{}"]))
        XCTAssertNil(WindowActivity.decode(["windowId": "", "payload": "{}"]))
        XCTAssertNil(WindowActivity.decode(["windowId": "w 1", "payload": "{}"]))
        XCTAssertNil(WindowActivity.decode(["windowId": "w1", "payload": ""]))
        XCTAssertNil(WindowActivity.decode(["windowId": 7, "payload": "{}"]))
        let long = String(repeating: "a", count: WindowActivity.maxWindowIdLength + 1)
        XCTAssertNil(WindowActivity.decode(["windowId": long, "payload": "{}"]))
        let big = String(repeating: "x", count: WindowActivity.maxPayloadBytes + 1)
        XCTAssertNil(WindowActivity.decode(["windowId": "w1", "payload": big]))
    }

    func testDecodeDefaultsAndClipsTheTitle() {
        XCTAssertEqual(WindowActivity.decode(["windowId": "w_1-a", "payload": "{}"])?.title, "")
        let title = String(repeating: "t", count: WindowActivity.maxTitleLength + 5)
        let decoded = WindowActivity.decode(["windowId": "w1", "payload": "{}", "title": title])
        XCTAssertEqual(decoded?.title.count, WindowActivity.maxTitleLength)
    }

    func testBookMapsWindowsAndSessionsOneToOne() {
        var book = WindowBook()
        book.connect(windowId: "main", session: "S0")
        book.connect(windowId: "w1", session: "S1")
        XCTAssertEqual(book.session(for: "w1"), "S1")
        XCTAssertEqual(book.windowId(for: "S0"), "main")
        // A session reused for another window drops the old mapping.
        book.connect(windowId: "w2", session: "S1")
        XCTAssertNil(book.session(for: "w1"))
        XCTAssertEqual(book.windowId(for: "S1"), "w2")
        // A window reconnecting in a new session moves.
        book.connect(windowId: "main", session: "S9")
        XCTAssertEqual(book.session(for: "main"), "S9")
    }

    func testDiscardReturnsTheClosedWindows() {
        var book = WindowBook()
        book.connect(windowId: "main", session: "S0")
        book.connect(windowId: "w2", session: "S2")
        book.connect(windowId: "w1", session: "S1")
        XCTAssertEqual(book.discard(["S1", "S2", "S7"]), ["w1", "w2"])
        XCTAssertNil(book.session(for: "w1"))
        XCTAssertEqual(book.session(for: "main"), "S0")
        XCTAssertEqual(book.discard(["S1"]), [])
    }

    func testDuplicateMainOnlyWhileTheMainSceneIsConnected() {
        var book = WindowBook()
        XCTAssertFalse(book.isDuplicateMain(session: "S5", connected: []))
        book.connect(windowId: "main", session: "S0")
        XCTAssertTrue(book.isDuplicateMain(session: "S5", connected: ["S0"]))
        XCTAssertFalse(book.isDuplicateMain(session: "S5", connected: []))
        XCTAssertFalse(book.isDuplicateMain(session: "S0", connected: ["S0"]))
    }

    func testPickerRequestIsAValidWindowThatRoundTrips() {
        let picker = WindowActivity.pickerRequest(random: "3F2A-91C0-uuid")
        XCTAssertEqual(picker.windowId, "p3F2A91C0uuid")
        XCTAssertEqual(picker.payload, #"{"kind":"picker"}"#)
        XCTAssertTrue(WindowActivity.isValid(picker))
        XCTAssertTrue(WindowActivity.isPicker(picker))
        XCTAssertFalse(WindowActivity.isPicker(request))
        XCTAssertEqual(WindowActivity.decode(WindowActivity.userInfo(picker)), picker)
        XCTAssertEqual(WindowActivity.decode(sessionInfo: WindowActivity.userInfo(picker)), picker)
        // Never the main window, never too long.
        XCTAssertTrue(WindowActivity.isValid(WindowActivity.pickerRequest(random: "")))
        let long = WindowActivity.pickerRequest(random: String(repeating: "a", count: 100))
        XCTAssertEqual(long.windowId.count, WindowActivity.maxWindowIdLength)
        let payload = try? JSONSerialization.jsonObject(with: Data(picker.payload.utf8)) as? [String: String]
        XCTAssertEqual(payload, ["kind": "picker"])
    }

    func testPhaseRawValuesMatchTheJsEvents() {
        XCTAssertEqual(
            [WindowPhase.active, .inactive, .background, .closed].map(\.rawValue),
            ["active", "inactive", "background", "closed"]
        )
    }
}
