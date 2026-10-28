import Foundation
import XCTest
@testable import NativeLogic

final class LaunchTests: XCTestCase {
    func testPropertiesParsesRecognizedArguments() {
        let properties = LaunchArguments.properties([
            "app",
            "-autoConnect",
            "100.64.0.7",
            "-devHarness",
            "ping",
        ])

        XCTAssertEqual(properties["autoConnect"], "100.64.0.7")
        XCTAssertEqual(properties["devHarness"], "ping")
        XCTAssertEqual(properties.count, 2)
    }

    func testPropertiesRejectsMissingOrFlagValues() {
        XCTAssertNil(LaunchArguments.properties(["app", "-autoConnect"])["autoConnect"])

        let properties = LaunchArguments.properties(["-autoConnect", "-devHarness", "x"])
        XCTAssertNil(properties["autoConnect"])
        XCTAssertEqual(properties["devHarness"], "x")
    }

    func testPropertiesUsesTheFirstOccurrence() {
        let properties = LaunchArguments.properties([
            "-devHarness",
            "first",
            "-devHarness",
            "second",
        ])

        XCTAssertEqual(properties["devHarness"], "first")
    }

    func testPropertiesIgnoresRemovedHubAndFakeDirectoryArguments() {
        let properties = LaunchArguments.properties([
            "app",
            "-hubUrl",
            "http://100.64.0.10:7300",
            "-fakeDirectory",
            "dir.json",
        ])

        XCTAssertEqual(properties, [:])
    }

    func testDataDirectoryIsUnderRemoteScreenCore() {
        let url = LaunchDataDir.url(appSupport: URL(fileURLWithPath: "/tmp/app-support"))
        XCTAssertTrue(url.path.hasSuffix("RemoteScreen/core"))
    }
}
