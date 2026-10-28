import CoreGraphics
import XCTest
@testable import NativeLogic

final class ViewportGeometryTests: XCTestCase {
    private let viewSize = CGSize(width: 1000, height: 600)
    private let contentRect = CGRect(x: 0, y: 50, width: 1000, height: 500)

    func testVisibleWholeAtOneX() {
        let visible = ViewportGeometry.visibleRect(viewport: viewport(zoom: 1),
                                                   viewSize: viewSize, keyboardInset: 0)
        assertRect(visible, CGRect(x: 0, y: 0, width: 1, height: 1))
    }

    func testVisibleLetterboxAxisWiderThanInverseZoom() {
        let visible = ViewportGeometry.visibleRect(viewport: viewport(zoom: 2,
                                                                        offset: CGPoint(x: 0.25, y: 0.25)),
                                                   viewSize: viewSize, keyboardInset: 0)
        assertRect(visible, CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.6))
    }

    func testVisibleMinusKeyboard() {
        let visible = ViewportGeometry.visibleRect(viewport: viewport(zoom: 2,
                                                                        offset: CGPoint(x: 0.25, y: 0.25)),
                                                   viewSize: viewSize, keyboardInset: 100)
        assertRect(visible, CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.5))
    }

    func testVisibleClampedAtEdges() {
        let visible = ViewportGeometry.visibleRect(viewport: viewport(zoom: 4,
                                                                        offset: CGPoint(x: 0.75, y: 0.75)),
                                                   viewSize: viewSize, keyboardInset: 0)
        XCTAssertEqual(visible.minX, 0.75, accuracy: 0.000001)
        XCTAssertEqual(visible.maxX, 1, accuracy: 0.000001)
        XCTAssertEqual(visible.minY, 0.725, accuracy: 0.000001)
        XCTAssertEqual(visible.maxY, 1, accuracy: 0.000001)
    }

    func testVisibleEmptyContent() {
        let viewport = SessionViewport(contentRect: .zero, zoom: 2, offset: CGPoint(x: 0.25, y: 0.25))
        let visible = ViewportGeometry.visibleRect(viewport: viewport,
                                                   viewSize: viewSize, keyboardInset: 0)
        assertRect(visible, CGRect(x: 0, y: 0, width: 1, height: 1))
    }

    func testViewPixelsScale3() {
        let visible = CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.6)
        let pixels = ViewportGeometry.viewPixels(visible: visible,
                                                 viewport: viewport(zoom: 2),
                                                 screenScale: 3)
        XCTAssertEqual(pixels.width, 3000)
        XCTAssertEqual(pixels.height, 1800)
    }

    func testSourceSanitizes() {
        assertRect(ViewportGeometry.source(x: .nan, y: 0, w: 1, h: 1),
                   CGRect(x: 0, y: 0, width: 1, height: 1))
        assertRect(ViewportGeometry.source(x: 0, y: 0, w: 0, h: 1),
                   CGRect(x: 0, y: 0, width: 1, height: 1))
        assertRect(ViewportGeometry.source(x: 0.25, y: 0.2, w: 0.5, h: 0.6),
                   CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.6))
        assertRect(ViewportGeometry.source(x: 0.8, y: 0, w: 0.5, h: 1),
                   CGRect(x: 0.8, y: 0, width: 0.2, height: 1))
    }

    func testIsFullTolerance() {
        XCTAssertTrue(ViewportGeometry.isFull(CGRect(x: 0, y: 0, width: 1, height: 1)))
        XCTAssertTrue(ViewportGeometry.isFull(CGRect(x: 0.00005, y: 0,
                                                     width: 0.99995, height: 1)))
        XCTAssertFalse(ViewportGeometry.isFull(CGRect(x: 0.25, y: 0.2,
                                                      width: 0.5, height: 0.6)))
    }

    func testCropFrameInsideContent() {
        let frame = ViewportGeometry.cropFrame(contentRect: contentRect,
                                               src: CGRect(x: 0.25, y: 0.2,
                                                           width: 0.5, height: 0.6))
        assertRect(frame, CGRect(x: 250, y: 150, width: 500, height: 300))
    }

    func testLogicalSize() {
        let crop = CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.6)
        XCTAssertEqual(ViewportGeometry.logicalVideoSize(decoded: CGSize(width: 1000, height: 600),
                                                         src: crop),
                       CGSize(width: 2000, height: 1000))
        XCTAssertEqual(ViewportGeometry.logicalVideoSize(decoded: CGSize(width: 1000, height: 600),
                                                         src: CGRect(x: 0, y: 0, width: 1, height: 1)),
                       CGSize(width: 1000, height: 600))
    }

    func testCursorMapping() {
        let crop = CGRect(x: 0.25, y: 0.2, width: 0.5, height: 0.6)
        assertPoint(ViewportGeometry.cursorInCrop(CGPoint(x: 0.5, y: 0.5), src: crop),
                    CGPoint(x: 0.5, y: 0.5))
        assertPoint(ViewportGeometry.cursorInCrop(CGPoint(x: 0.75, y: 0.8), src: crop),
                    CGPoint(x: 1, y: 1))
        XCTAssertNil(ViewportGeometry.cursorInCrop(CGPoint(x: 0.1, y: 0.5), src: crop))
        assertPoint(ViewportGeometry.cursorInCrop(CGPoint(x: 0.1, y: 0.5),
                                                  src: CGRect(x: 0, y: 0, width: 1, height: 1)),
                    CGPoint(x: 0.1, y: 0.5))
    }

    private func viewport(zoom: CGFloat, offset: CGPoint = .zero) -> SessionViewport {
        SessionViewport(contentRect: contentRect, zoom: zoom, offset: offset)
    }

    private func assertRect(_ actual: CGRect, _ expected: CGRect,
                            file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.minX, expected.minX, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.minY, expected.minY, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.width, expected.width, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.height, expected.height, accuracy: 0.000001, file: file, line: line)
    }

    private func assertPoint(_ actual: CGPoint?, _ expected: CGPoint,
                             file: StaticString = #filePath, line: UInt = #line) {
        guard let actual else {
            XCTFail("expected a point", file: file, line: line)
            return
        }
        XCTAssertEqual(actual.x, expected.x, accuracy: 0.000001, file: file, line: line)
        XCTAssertEqual(actual.y, expected.y, accuracy: 0.000001, file: file, line: line)
    }
}
