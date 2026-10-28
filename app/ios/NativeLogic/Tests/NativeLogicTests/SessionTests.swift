import CoreGraphics
import XCTest
@testable import NativeLogic

final class SessionTests: XCTestCase {
    func testSessionIdParsing() {
        XCTAssertEqual(SessionIdParse.parse("123"), 123)
        XCTAssertEqual(SessionIdParse.parse("18446744073709551615"), UInt64.max)
        XCTAssertEqual(SessionIdParse.parse("0"), 0)
        for value in ["", "-1", "+1", "1e3", " 1", "1\n", "１２３", "000000000000000000001", "18446744073709551616"] {
            XCTAssertNil(SessionIdParse.parse(value), value)
        }
    }

    func testCodecDetection() {
        XCTAssertEqual(SessionCodec.detect(vpsLen: 1, spsLen: 1, ppsLen: 1), .hevc)
        XCTAssertEqual(SessionCodec.detect(vpsLen: 0, spsLen: 1, ppsLen: 1), .h264)
        XCTAssertEqual(SessionCodec.detect(vpsLen: 1, spsLen: 0, ppsLen: 1), .noParameterSets)
        XCTAssertEqual(SessionCodec.detect(vpsLen: 0, spsLen: 0, ppsLen: 0), .noParameterSets)
    }

    func testEpochTracker() {
        var tracker = SessionEpochTracker()
        XCTAssertEqual(tracker.observe(3), .first)
        XCTAssertEqual(tracker.observe(3), .same)
        XCTAssertEqual(tracker.observe(4), .changed(from: 3, to: 4))
    }

    func testContentRect() {
        let landscape = SessionContentRect.fit(video: CGSize(width: 1920, height: 1080),
                                                in: CGSize(width: 2556, height: 1179))
        XCTAssertEqual(landscape.width, 2096, accuracy: 1)
        XCTAssertEqual(landscape.minX, 230, accuracy: 1)
        let portrait = SessionContentRect.fit(video: CGSize(width: 1920, height: 1080),
                                               in: CGSize(width: 1179, height: 2556))
        XCTAssertEqual(portrait.height, 663.1875, accuracy: 1)
        XCTAssertEqual(portrait.midY, 1278, accuracy: 1)
        XCTAssertEqual(SessionContentRect.fit(video: .zero, in: CGSize(width: 1, height: 1)), .zero)
        XCTAssertEqual(SessionContentRect.fit(video: CGSize(width: 1, height: 1), in: .zero), .zero)
    }

}
