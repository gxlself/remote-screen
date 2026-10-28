import XCTest
@testable import NativeLogic

final class CommonTests: XCTestCase {
    func testNormalMarkerSortsKeysAndEscapesStrings() {
        let marker = RsMarkerLine.format(
            tag: "TEST",
            fields: [
                "quote": "a\"b",
                "text": "line\nnext",
                "array": ["first", "second"],
            ]
        )

        XCTAssertEqual(
            marker,
            "RS-TEST {\"array\":[\"first\",\"second\"],\"quote\":\"a\\\"b\",\"text\":\"line\\nnext\"}"
        )
    }

    func testInvalidTagUsesTruncatedTagField() {
        let marker = RsMarkerLine.format(
            tag: "bad tag with a value longer than thirty-two characters",
            fields: ["ignored": true]
        )

        XCTAssertEqual(marker, "RS-INVALID {\"tag\":\"bad tag with a value longer than\"}")
    }

    func testNonFiniteDoublesBecomeNull() {
        let marker = RsMarkerLine.format(
            tag: "NUMBERS",
            fields: ["infinity": Double.infinity, "nan": Double.nan]
        )

        XCTAssertEqual(marker, "RS-NUMBERS {\"infinity\":null,\"nan\":null}")
    }

    func testNestedArraysAreNormalizedRecursively() {
        let marker = RsMarkerLine.format(
            tag: "ARRAY",
            fields: ["value": [1, [true, "text"]]]
        )

        XCTAssertEqual(marker, "RS-ARRAY {\"value\":[1,[true,\"text\"]]}")
    }
}
