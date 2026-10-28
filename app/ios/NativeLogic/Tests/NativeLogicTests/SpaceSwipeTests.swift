import CoreGraphics
import XCTest
@testable import NativeLogic

final class SpaceSwipeTests: XCTestCase {
    private let width: CGFloat = 1000

    func testSmallMovesStayATapAndVerticalMovesAreLeftAlone() {
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: 10, y: 5)), .undecided)
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: -23, y: 0)), .undecided)
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: -30, y: 4)), .swipe)
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: 40, y: 10)), .swipe)
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: 10, y: 40)), .other)
        // Diagonal: horizontal has to win clearly.
        XCTAssertEqual(SpaceSwipe.classify(translation: CGPoint(x: 30, y: 28)), .other)
    }

    func testThePictureFollowsTheFingersUpToOneScreen() {
        XCTAssertEqual(SpaceSwipe.offset(translationX: -300, width: width, hasPrevious: true, hasNext: true), -300)
        XCTAssertEqual(SpaceSwipe.offset(translationX: 250, width: width, hasPrevious: true, hasNext: false), 250)
        XCTAssertEqual(SpaceSwipe.offset(translationX: -1500, width: width, hasPrevious: false, hasNext: true), -1000)
    }

    func testTheEndsResistLikeARubberBand() {
        // No computer to the left: dragging right only moves a little.
        let pulled = SpaceSwipe.offset(translationX: 300, width: width, hasPrevious: false, hasNext: true)
        XCTAssertGreaterThan(pulled, 0)
        XCTAssertLessThan(pulled, 150)
        let further = SpaceSwipe.offset(translationX: 900, width: width, hasPrevious: false, hasNext: true)
        XCTAssertGreaterThan(further, pulled)
        XCTAssertLessThan(further, width / 2)
        // No computer to the right: the same toward the left.
        let left = SpaceSwipe.offset(translationX: -300, width: width, hasPrevious: true, hasNext: false)
        XCTAssertEqual(left, -pulled, accuracy: 0.001)
        XCTAssertEqual(SpaceSwipe.offset(translationX: 100, width: 0, hasPrevious: true, hasNext: true), 0)
    }

    func testPastAThirdSwitchesOtherwiseItSpringsBack() {
        XCTAssertEqual(SpaceSwipe.decide(translationX: -340, velocityX: 0, width: width,
                                         hasPrevious: true, hasNext: true), .next)
        XCTAssertEqual(SpaceSwipe.decide(translationX: 340, velocityX: 50, width: width,
                                         hasPrevious: true, hasNext: true), .previous)
        XCTAssertNil(SpaceSwipe.decide(translationX: -320, velocityX: 0, width: width,
                                       hasPrevious: true, hasNext: true))
    }

    func testAFlickSwitchesFromAShortDistance() {
        XCTAssertEqual(SpaceSwipe.decide(translationX: -60, velocityX: -900, width: width,
                                         hasPrevious: false, hasNext: true), .next)
        // Not past the slop: still a tap.
        XCTAssertNil(SpaceSwipe.decide(translationX: -10, velocityX: -900, width: width,
                                       hasPrevious: false, hasNext: true))
        // Flung back toward the start: springs back even past the third.
        XCTAssertNil(SpaceSwipe.decide(translationX: -500, velocityX: 900, width: width,
                                       hasPrevious: true, hasNext: true))
    }

    func testTheEndsNeverSwitchOrWrap() {
        XCTAssertNil(SpaceSwipe.decide(translationX: 800, velocityX: 1200, width: width,
                                       hasPrevious: false, hasNext: true))
        XCTAssertNil(SpaceSwipe.decide(translationX: -800, velocityX: -1200, width: width,
                                       hasPrevious: true, hasNext: false))
    }

    func testNeighboursFollowTheListWithoutWrapping() {
        let ids = ["a", "b", "c"]
        XCTAssertTrue(SpaceSwipe.neighbours(of: "a", in: ids) == (nil, "b"))
        XCTAssertTrue(SpaceSwipe.neighbours(of: "b", in: ids) == ("a", "c"))
        XCTAssertTrue(SpaceSwipe.neighbours(of: "c", in: ids) == ("b", nil))
        XCTAssertTrue(SpaceSwipe.neighbours(of: "x", in: ids) == (nil, nil))
        XCTAssertTrue(SpaceSwipe.neighbours(of: "a", in: ["a"]) == (nil, nil))
    }

    func testFinalOffsetMovesAWholeScreen() {
        XCTAssertEqual(SpaceSwipe.finalOffset(.next, width: width), -width)
        XCTAssertEqual(SpaceSwipe.finalOffset(.previous, width: width), width)
        XCTAssertEqual(SpaceSwipe.finalOffset(nil, width: width), 0)
        XCTAssertEqual(SpaceSwipeDirection.next.rawValue, "next")
        XCTAssertEqual(SpaceSwipeDirection(rawValue: "previous"), .previous)
    }
}

final class SessionSnapshotLRUTests: XCTestCase {
    func testKeepsAtMostSixAndDropsTheLeastRecentlyUsed() {
        var lru = SessionSnapshotLRU<String>()
        XCTAssertEqual(lru.capacity, 6)
        for key in ["1", "2", "3", "4", "5", "6"] {
            XCTAssertEqual(lru.insert(key), [])
        }
        // Showing 1 beside the picture keeps it.
        lru.touch("1")
        XCTAssertEqual(lru.insert("7"), ["2"])
        XCTAssertEqual(lru.order, ["3", "4", "5", "6", "1", "7"])
        // Storing a session again refreshes it instead of adding a copy.
        XCTAssertEqual(lru.insert("3"), [])
        XCTAssertEqual(lru.order, ["4", "5", "6", "1", "7", "3"])
    }

    func testForgetsEndedSessions() {
        var lru = SessionSnapshotLRU<String>(capacity: 3)
        _ = lru.insert("a")
        _ = lru.insert("b")
        _ = lru.insert("c")
        XCTAssertTrue(lru.remove("b"))
        XCTAssertFalse(lru.remove("b"))
        XCTAssertEqual(lru.retain(only: ["c", "z"]), ["a"])
        XCTAssertEqual(lru.order, ["c"])
        XCTAssertTrue(lru.contains("c"))
        XCTAssertFalse(lru.contains("a"))
        lru.touch("missing")
        XCTAssertEqual(lru.order, ["c"])
    }

    func testScalesDownToTheScreenAndAtMost1920Wide() {
        XCTAssertTrue(SessionSnapshotLRU<String>.scaledSize(width: 3840, height: 2160, limit: 2732) == (1920, 1080))
        XCTAssertTrue(SessionSnapshotLRU<String>.scaledSize(width: 2560, height: 1600, limit: 1366) == (1366, 854))
        // Never enlarged.
        XCTAssertTrue(SessionSnapshotLRU<String>.scaledSize(width: 1280, height: 800, limit: 2732) == (1280, 800))
        XCTAssertTrue(SessionSnapshotLRU<String>.scaledSize(width: 0, height: 800, limit: 2732) == (0, 0))
    }
}
