import Foundation
import XCTest
@testable import NativeLogic

final class FilesTests: XCTestCase {
    func testUniqueNameAddsSuffixBeforeExtension() {
        XCTAssertEqual(
            UniqueName.next(name: "a.txt", taken: ["a.txt"]),
            "a (1).txt"
        )
        XCTAssertEqual(
            UniqueName.next(
                name: "a.txt",
                taken: ["a.txt", "a (1).txt"]
            ),
            "a (2).txt"
        )
        XCTAssertEqual(
            UniqueName.next(name: "a", taken: ["a"]),
            "a (1)"
        )
        XCTAssertEqual(
            UniqueName.next(name: ".hidden", taken: [".hidden"]),
            ".hidden (1)"
        )
    }

    func testIncomingURLOnlyAcceptsFileScheme() {
        XCTAssertTrue(IncomingURL.isFile(URL(string: "file:///x")!))
        XCTAssertTrue(IncomingURL.isFile(URL(string: "FILE:///x")!))
        XCTAssertFalse(IncomingURL.isFile(URL(string: "rsp://pair?x=1")!))
        XCTAssertFalse(IncomingURL.isFile(URL(string: "https://hs.example.com/x")!))
    }

    func testBackgroundTaskGateTransitions() {
        var gate = BackgroundTaskGate()
        XCTAssertEqual(gate.set(active: true), .begin)
        XCTAssertEqual(gate.set(active: true), .none)
        XCTAssertEqual(gate.set(active: false), .end)
        XCTAssertEqual(gate.expired(), .none)
        XCTAssertEqual(gate.set(active: false), .none)
        XCTAssertEqual(gate.set(active: true), .begin)
    }

    func testBackgroundTaskGateExpirationResetsState() {
        var gate = BackgroundTaskGate()
        XCTAssertEqual(gate.set(active: true), .begin)
        XCTAssertEqual(gate.expired(), .end)
        XCTAssertEqual(gate.set(active: false), .none)
        XCTAssertEqual(gate.set(active: true), .begin)
    }
}
