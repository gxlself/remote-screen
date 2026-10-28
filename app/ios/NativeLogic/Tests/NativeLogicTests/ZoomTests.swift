import CoreGraphics
import XCTest
@testable import NativeLogic

final class ZoomTests: XCTestCase {
    func testZoomAroundCenterAndCorner() {
        let center = SessionViewport().zoomed(by: 2, anchor: CGPoint(x: 0.5, y: 0.5))
        XCTAssertEqual(center.zoom, 2)
        XCTAssertEqual(center.visibleRect, CGRect(x: 0.25, y: 0.25, width: 0.5, height: 0.5))

        let corner = SessionViewport().zoomed(by: 2, anchor: .zero)
        XCTAssertEqual(corner.visibleRect, CGRect(x: 0, y: 0, width: 0.5, height: 0.5))
    }

    func testZoomAndPanClamp() {
        let zoomed = SessionViewport().zoomed(by: 0.2, anchor: .zero)
        XCTAssertEqual(zoomed.zoom, 1)
        let panned = SessionViewport(zoom: 2).panned(by: CGPoint(x: 2, y: -1))
        XCTAssertEqual(panned.offset, CGPoint(x: 0.5, y: 0))
    }

    func testFollowCursor() {
        let viewport = SessionViewport(zoom: 2, offset: .zero)
        let followed = SessionZoomMath.followCursor(cursor: CGPoint(x: 0.49, y: 0.2),
                                                     viewport: viewport, margin: 0.1)
        XCTAssertGreaterThan(followed.offset.x, 0)
        XCTAssertLessThanOrEqual(followed.visibleRect.maxX - 0.49, followed.visibleRect.width * 0.1 + 0.0001)
    }

    func testKeyboardTarget() {
        let target = SessionZoomMath.keyboardTarget(cursor: CGPoint(x: 0.5, y: 0.8),
                                                     inset: 336, viewSize: CGSize(width: 390, height: 844),
                                                     minZoom: 1.7)
        XCTAssertEqual(target.zoom, 1.7, accuracy: 0.0001)
        let cursorY = (0.8 - target.offset.y) * target.zoom * 844
        XCTAssertEqual(cursorY, (844 - 336) / 2, accuracy: 1)
        let existing = SessionZoomMath.keyboardTarget(cursor: CGPoint(x: 0.5, y: 0.8),
                                                       inset: 336, viewSize: CGSize(width: 390, height: 844),
                                                       minZoom: 1.7, currentZoom: 2.5)
        XCTAssertEqual(existing.zoom, 2.5, accuracy: 0.0001)
    }

    func testMagnifierGeometryAndTapMapping() {
        let first = SessionMagnifierGeometry.frame(finger: CGPoint(x: 200, y: 300),
                                                    viewBounds: CGRect(x: 0, y: 0, width: 390, height: 844),
                                                    diameter: 120, offset: CGPoint(x: 0, y: -80))
        XCTAssertEqual(first.midX, 200, accuracy: 0.001)
        XCTAssertEqual(first.midY, 220, accuracy: 0.001)
        let top = SessionMagnifierGeometry.frame(finger: CGPoint(x: 200, y: 40),
                                                  viewBounds: CGRect(x: 0, y: 0, width: 390, height: 844),
                                                  diameter: 120, offset: CGPoint(x: 0, y: -80))
        XCTAssertGreaterThan(top.minY, 0)
        XCTAssertTrue(CGRect(x: 0, y: 0, width: 390, height: 844).contains(top))

        let viewport = SessionViewport(contentRect: CGRect(x: 0, y: 0, width: 390, height: 844), zoom: 2)
        let normalized = TouchTapMapper.normalized(viewPoint: CGPoint(x: 195, y: 422), viewport: viewport)
        XCTAssertEqual(normalized, CGPoint(x: 0.25, y: 0.25))
        XCTAssertEqual(TouchTapMapper.normalized(viewPoint: CGPoint(x: -10, y: 900), viewport: viewport),
                       CGPoint(x: 0, y: 0.5))
    }
}
