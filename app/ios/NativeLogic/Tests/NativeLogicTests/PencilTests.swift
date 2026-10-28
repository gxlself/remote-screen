import CoreGraphics
import XCTest
@testable import NativeLogic

final class PencilTests: XCTestCase {
    private let viewport = SessionViewport(contentRect: CGRect(x: 100, y: 0, width: 800, height: 450))

    func testBeganMapsToNormalizedPositionAndPressesImmediately() {
        var mapper = PencilStrokeMapper()

        XCTAssertEqual(mapper.began(viewPoint: CGPoint(x: 500, y: 225), viewport: viewport, time: 0), [
            .move(CGPoint(x: 0.5, y: 0.5)),
            .down(CGPoint(x: 0.5, y: 0.5), clicks: 1),
        ])
        XCTAssertTrue(mapper.isDown)
    }

    func testHoverDeduplicatesAndResetsAfterEnding() {
        var mapper = PencilStrokeMapper()
        let center = CGPoint(x: 500, y: 225)

        XCTAssertEqual(mapper.hover(viewPoint: center, viewport: viewport), [.move(CGPoint(x: 0.5, y: 0.5))])
        XCTAssertEqual(mapper.hover(viewPoint: CGPoint(x: 500.3, y: 225), viewport: viewport), [])
        XCTAssertEqual(mapper.hover(viewPoint: CGPoint(x: 501, y: 225), viewport: viewport),
                       [.move(CGPoint(x: 0.50125, y: 0.5))])
        mapper.hoverEnded()
        XCTAssertEqual(mapper.hover(viewPoint: CGPoint(x: 501, y: 225), viewport: viewport),
                       [.move(CGPoint(x: 0.50125, y: 0.5))])

        _ = mapper.began(viewPoint: center, viewport: viewport, time: 0)
        XCTAssertEqual(mapper.hover(viewPoint: CGPoint(x: 600, y: 225), viewport: viewport), [])
    }

    func testTapWithinDragSlopReleasesAtDownPoint() {
        var mapper = PencilStrokeMapper()
        let down = CGPoint(x: 500, y: 225)
        let up = CGPoint(x: 502, y: 225)

        _ = mapper.began(viewPoint: down, viewport: viewport, time: 0)
        XCTAssertEqual(mapper.moved(viewPoint: up, viewport: viewport), [])
        XCTAssertEqual(mapper.ended(viewPoint: up, viewport: viewport),
                       [.up(CGPoint(x: 0.5, y: 0.5), clicks: 1)])
        XCTAssertFalse(mapper.isDown)
    }

    func testDragCrossingSlopContinuesToEmitMoves() {
        var mapper = PencilStrokeMapper()
        let down = CGPoint(x: 500, y: 225)

        _ = mapper.began(viewPoint: down, viewport: viewport, time: 0)
        XCTAssertEqual(mapper.moved(viewPoint: CGPoint(x: 510, y: 225), viewport: viewport),
                       [.move(CGPoint(x: 0.5125, y: 0.5))])
        XCTAssertEqual(mapper.moved(viewPoint: CGPoint(x: 511, y: 225), viewport: viewport),
                       [.move(CGPoint(x: 0.51375, y: 0.5))])
        XCTAssertEqual(mapper.ended(viewPoint: CGPoint(x: 511, y: 225), viewport: viewport), [
            .move(CGPoint(x: 0.51375, y: 0.5)),
            .up(CGPoint(x: 0.51375, y: 0.5), clicks: 1),
        ])
    }

    func testClickCounterCapsAtThreeAndExpires() {
        var mapper = PencilStrokeMapper()
        let point = CGPoint(x: 500, y: 225)

        XCTAssertEqual(clicks(from: &mapper, point: point, time: 0), 1)
        XCTAssertEqual(clicks(from: &mapper, point: point, time: 0.2), 2)
        XCTAssertEqual(clicks(from: &mapper, point: point, time: 0.4), 3)
        XCTAssertEqual(clicks(from: &mapper, point: point, time: 0.6), 3)
        XCTAssertEqual(clicks(from: &mapper, point: point, time: 1.1), 1)
    }

    func testDuplicateAndOutOfOrderLifecycleCallsAreIgnored() {
        var mapper = PencilStrokeMapper()
        let point = CGPoint(x: 500, y: 225)

        XCTAssertEqual(mapper.moved(viewPoint: point, viewport: viewport), [])
        XCTAssertEqual(mapper.ended(viewPoint: point, viewport: viewport), [])
        XCTAssertEqual(mapper.cancelled(), [])

        _ = mapper.began(viewPoint: point, viewport: viewport, time: 0)
        XCTAssertEqual(mapper.began(viewPoint: CGPoint(x: 600, y: 225), viewport: viewport, time: 0.1), [])
        XCTAssertEqual(mapper.cancelled(), [.up(CGPoint(x: 0.5, y: 0.5), clicks: 1)])
        XCTAssertFalse(mapper.isDown)
    }

    func testCancelledUsesLastEmittedPositionAndResetClearsClicks() {
        var mapper = PencilStrokeMapper()
        let point = CGPoint(x: 500, y: 225)

        _ = mapper.began(viewPoint: point, viewport: viewport, time: 0)
        _ = mapper.moved(viewPoint: CGPoint(x: 510, y: 225), viewport: viewport)
        XCTAssertEqual(mapper.cancelled(), [.up(CGPoint(x: 0.5125, y: 0.5), clicks: 1)])

        _ = mapper.began(viewPoint: point, viewport: viewport, time: 0.2)
        _ = mapper.ended(viewPoint: point, viewport: viewport)
        XCTAssertEqual(mapper.reset(), [])
        XCTAssertEqual(mapper.began(viewPoint: point, viewport: viewport, time: 0.3), [
            .move(CGPoint(x: 0.5, y: 0.5)),
            .down(CGPoint(x: 0.5, y: 0.5), clicks: 1),
        ])
    }

    func testZoomedViewportKeepsCenterAtHalf() {
        let zoomed = SessionViewport(contentRect: CGRect(x: 100, y: 0, width: 800, height: 450),
                                     zoom: 2, offset: CGPoint(x: 0.25, y: 0.25))
        var mapper = PencilStrokeMapper()

        XCTAssertEqual(mapper.began(viewPoint: CGPoint(x: 500, y: 225), viewport: zoomed, time: 0), [
            .move(CGPoint(x: 0.5, y: 0.5)),
            .down(CGPoint(x: 0.5, y: 0.5), clicks: 1),
        ])
    }

    private func clicks(from mapper: inout PencilStrokeMapper, point: CGPoint, time: TimeInterval) -> UInt8 {
        let actions = mapper.began(viewPoint: point, viewport: viewport, time: time)
        _ = mapper.ended(viewPoint: point, viewport: viewport)
        guard let last = actions.last, case let .down(_, clicks) = last else { return 0 }
        return clicks
    }
}
