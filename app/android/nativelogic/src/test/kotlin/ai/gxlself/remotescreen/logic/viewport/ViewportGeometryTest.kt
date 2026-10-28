package ai.gxlself.remotescreen.logic.viewport

import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.logic.session.ContentRect
import ai.gxlself.remotescreen.logic.session.RectF4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ViewportGeometryTest {
    @Test
    fun fixtureMatchesContentRectFit() {
        assertEquals(
            RectF4(0f, 50f, 1000f, 500f),
            ContentRect.fit(2000f, 1000f, VIEW_W, VIEW_H),
        )
    }

    @Test
    fun visibleWholeAtOneX() {
        assertRect(FULL, ViewportGeometry.visibleRect(vp(1f, 0f, 0f), VIEW_W, VIEW_H, 0f))
    }

    @Test
    fun visibleLetterboxAxisWiderThanInverseZoom() {
        assertRect(
            GRect(0.25f, 0.2f, 0.5f, 0.6f),
            ViewportGeometry.visibleRect(vp(2f, 0.25f, 0.25f), VIEW_W, VIEW_H, 0f),
        )
    }

    @Test
    fun visibleMinusKeyboard() {
        assertRect(
            GRect(0.25f, 0.2f, 0.5f, 0.5f),
            ViewportGeometry.visibleRect(vp(2f, 0.25f, 0.25f), VIEW_W, VIEW_H, 100f),
        )
    }

    @Test
    fun keyboardInsetIsClampedAndNonFiniteMeansZero() {
        val viewport = vp(2f, 0.25f, 0.25f)
        for (inset in NON_FINITE + listOf(-100f)) {
            assertRect(
                GRect(0.25f, 0.2f, 0.5f, 0.6f),
                ViewportGeometry.visibleRect(viewport, VIEW_W, VIEW_H, inset),
            )
        }
        for (inset in listOf(VIEW_H, VIEW_H + 100f)) {
            assertRect(
                GRect(0.25f, 0.2f, 0.5f, 0f),
                ViewportGeometry.visibleRect(viewport, VIEW_W, VIEW_H, inset),
            )
        }
    }

    @Test
    fun visibleClampedAtEdges() {
        assertRect(
            GRect(0.75f, 0.725f, 0.25f, 0.275f),
            ViewportGeometry.visibleRect(vp(4f, 0.75f, 0.75f), VIEW_W, VIEW_H, 0f),
        )
    }

    @Test
    fun visibleEmptyContent() {
        assertRect(
            FULL,
            ViewportGeometry.visibleRect(
                SessionViewport(GRect.ZERO, 1f, GPoint.ZERO),
                VIEW_W,
                VIEW_H,
                0f,
            ),
        )
        assertRect(
            FULL,
            ViewportGeometry.visibleRect(vp(1f, 0f, 0f), Float.NaN, VIEW_H, 0f),
        )
    }

    @Test
    fun visibleInvalidBoundsAndContentReturnFullDisplay() {
        val viewport = vp(2f, 0.25f, 0.25f)
        for (dimension in NON_FINITE + listOf(0f, -1f)) {
            assertRect(FULL, ViewportGeometry.visibleRect(viewport, dimension, VIEW_H, 0f))
            assertRect(FULL, ViewportGeometry.visibleRect(viewport, VIEW_W, dimension, 0f))
            for (content in listOf(CONTENT.copy(w = dimension), CONTENT.copy(h = dimension))) {
                assertRect(
                    FULL,
                    ViewportGeometry.visibleRect(SessionViewport(content), VIEW_W, VIEW_H, 0f),
                )
            }
        }
        for (value in NON_FINITE) {
            for (content in listOf(CONTENT.copy(x = value), CONTENT.copy(y = value))) {
                assertRect(
                    FULL,
                    ViewportGeometry.visibleRect(SessionViewport(content), VIEW_W, VIEW_H, 0f),
                )
            }
        }
    }

    @Test
    fun viewPixelsNoDensity() {
        val zoomed = vp(2f, 0.25f, 0.25f)
        val visible = ViewportGeometry.visibleRect(zoomed, VIEW_W, VIEW_H, 0f)
        assertEquals(PixelSize(1000, 600), ViewportGeometry.viewPixels(visible, zoomed))

        val oneX = vp(1f, 0f, 0f)
        val fullVisible = ViewportGeometry.visibleRect(oneX, VIEW_W, VIEW_H, 0f)
        assertEquals(PixelSize(1000, 500), ViewportGeometry.viewPixels(fullVisible, oneX))
    }

    @Test
    fun nonFiniteGeometryFallsBackSafely() {
        for (value in NON_FINITE) {
            for (src in listOf(
                FULL.copy(x = value),
                FULL.copy(y = value),
                FULL.copy(w = value),
                FULL.copy(h = value),
            )) {
                assertEquals(FULL, ViewportGeometry.source(src.x, src.y, src.w, src.h))
                assertFalse(ViewportGeometry.isFull(src))
                assertEquals(GRect.ZERO, ViewportGeometry.cropFrame(CONTENT, src))
                assertEquals(PixelSize(1000, 600), ViewportGeometry.logicalVideoSize(1000, 600, src))
                assertNull(ViewportGeometry.cursorInCrop(GPoint(0.5f, 0.5f), src))
                assertEquals(GRect.ZERO, ViewportGeometry.cropFrame(src, FULL))
            }
            assertNull(ViewportGeometry.cursorInCrop(GPoint(value, 0.5f), FULL))
            assertNull(ViewportGeometry.cursorInCrop(GPoint(0.5f, value), FULL))
        }
    }

    @Test
    fun viewPixelsFloorsAndSanitizesEachAxis() {
        val viewport = vp(1f, 0f, 0f)
        assertEquals(
            PixelSize(333, 166),
            ViewportGeometry.viewPixels(GRect(0f, 0f, 0.3339f, 0.3339f), viewport),
        )
        for (value in listOf(-1f)) {
            assertEquals(
                PixelSize(0, 500),
                ViewportGeometry.viewPixels(FULL.copy(w = value), viewport),
            )
            assertEquals(
                PixelSize(1000, 0),
                ViewportGeometry.viewPixels(FULL.copy(h = value), viewport),
            )
            assertEquals(
                PixelSize(0, 500),
                ViewportGeometry.viewPixels(FULL, SessionViewport(CONTENT.copy(w = value))),
            )
        }
        for (value in NON_FINITE) {
            assertEquals(PixelSize(0, 0), ViewportGeometry.viewPixels(FULL.copy(w = value), viewport))
            assertEquals(PixelSize(0, 0), ViewportGeometry.viewPixels(FULL.copy(h = value), viewport))
            assertEquals(
                PixelSize(0, 0),
                ViewportGeometry.viewPixels(FULL, SessionViewport(CONTENT.copy(w = value))),
            )
        }
        assertEquals(
            PixelSize(0, 1000),
            ViewportGeometry.viewPixels(FULL.copy(w = Float.MAX_VALUE), vp(2f, 0f, 0f)),
        )
        assertEquals(
            PixelSize(0, 0),
            ViewportGeometry.viewPixels(FULL.copy(x = Float.NaN), viewport),
        )
        assertEquals(
            PixelSize(1000, 500),
            ViewportGeometry.viewPixels(FULL.copy(x = Float.MAX_VALUE), viewport),
        )
        assertEquals(GRect.ZERO, ViewportGeometry.cropFrame(GRect(0f, 0f, -1f, 1f), FULL))
    }

    @Test
    fun sourceSanitizes() {
        assertEquals(FULL, ViewportGeometry.source(Float.NaN, 0f, 1f, 1f))
        assertEquals(FULL, ViewportGeometry.source(0f, 0f, 0f, 1f))
        assertRect(
            GRect(0.25f, 0.2f, 0.5f, 0.6f),
            ViewportGeometry.source(0.25f, 0.2f, 0.5f, 0.6f),
        )
        assertRect(GRect(0.8f, 0f, 0.2f, 1f), ViewportGeometry.source(0.8f, 0f, 0.5f, 1f))
        assertRect(GRect(0f, 0f, 0.5f, 0.6f), ViewportGeometry.source(-1f, -1f, 0.5f, 0.6f))
        assertRect(GRect(1f, 1f, 0f, 0f), ViewportGeometry.source(2f, 2f, 0.5f, 0.6f))
        assertEquals(FULL, ViewportGeometry.source(0f, 0f, 1f, -1f))
    }

    @Test
    fun isFullTolerance() {
        assertTrue(ViewportGeometry.isFull(FULL))
        assertTrue(ViewportGeometry.isFull(GRect(0.00005f, 0f, 0.99995f, 1f)))
        assertFalse(ViewportGeometry.isFull(GRect(0.25f, 0.2f, 0.5f, 0.6f)))
        assertFalse(ViewportGeometry.isFull(FULL.copy(x = 0.0002f)))
        assertFalse(ViewportGeometry.isFull(FULL.copy(y = 0.0002f)))
        assertFalse(ViewportGeometry.isFull(FULL.copy(w = 0.9998f)))
        assertFalse(ViewportGeometry.isFull(FULL.copy(h = 0.9998f)))
    }

    @Test
    fun cropFrameInsideContent() {
        assertRect(
            GRect(250f, 150f, 500f, 300f),
            ViewportGeometry.cropFrame(CONTENT, GRect(0.25f, 0.2f, 0.5f, 0.6f)),
        )
    }

    @Test
    fun logicalSize() {
        assertEquals(
            PixelSize(2000, 1000),
            ViewportGeometry.logicalVideoSize(1000, 600, GRect(0.25f, 0.2f, 0.5f, 0.6f)),
        )
        assertEquals(PixelSize(1000, 600), ViewportGeometry.logicalVideoSize(1000, 600, FULL))
        assertEquals(
            PixelSize(0, 600),
            ViewportGeometry.logicalVideoSize(0, 600, GRect(0.25f, 0.2f, 0.5f, 0.6f)),
        )
    }

    @Test
    fun cropAtDisplayEdgeAcceptsFloatRounding() {
        val src = GRect(0.8f, 0.8f, 0.2f, 0.2f)
        assertRect(GRect(800f, 450f, 200f, 100f), ViewportGeometry.cropFrame(CONTENT, src))
        assertEquals(PixelSize(2000, 1000), ViewportGeometry.logicalVideoSize(400, 200, src))
        assertPoint(GPoint(1f, 1f), ViewportGeometry.cursorInCrop(GPoint(1f, 1f), src))
    }

    @Test
    fun emptySourceStaysEmptyWhenMappedToContent() {
        val src = ViewportGeometry.source(2f, 0f, 0.5f, 1f)
        assertEquals(GRect.ZERO, ViewportGeometry.cropFrame(CONTENT, src))
        assertEquals(PixelSize(1000, 600), ViewportGeometry.logicalVideoSize(1000, 600, src))
        assertNull(ViewportGeometry.cursorInCrop(GPoint(1f, 0.5f), src))
    }

    @Test
    fun logicalSizeRoundsAndKeepsDecodedSizeForInvalidSources() {
        assertEquals(
            PixelSize(8, 6),
            ViewportGeometry.logicalVideoSize(3, 3, GRect(0f, 0f, 0.4f, 0.5f)),
        )
        for (src in listOf(GRect.ZERO, FULL.copy(w = -1f), FULL.copy(w = Float.MIN_VALUE))) {
            assertEquals(PixelSize(1000, 600), ViewportGeometry.logicalVideoSize(1000, 600, src))
        }
        assertEquals(
            PixelSize(1000, -1),
            ViewportGeometry.logicalVideoSize(1000, -1, GRect(0f, 0f, 0.5f, 0.5f)),
        )
    }

    @Test
    fun cursorMapping() {
        val src = GRect(0.25f, 0.2f, 0.5f, 0.6f)
        assertPoint(GPoint(0.5f, 0.5f), ViewportGeometry.cursorInCrop(GPoint(0.5f, 0.5f), src))
        assertPoint(GPoint(1f, 1f), ViewportGeometry.cursorInCrop(GPoint(0.75f, 0.8f), src))
        assertNull(ViewportGeometry.cursorInCrop(GPoint(0.1f, 0.5f), src))
        assertPoint(GPoint(0.1f, 0.5f), ViewportGeometry.cursorInCrop(GPoint(0.1f, 0.5f), FULL))
    }

    @Test
    fun cursorToleranceIncludesNearEdgesAndRejectsOutside() {
        val src = GRect(0.25f, 0.2f, 0.5f, 0.6f)
        assertPoint(
            GPoint.ZERO,
            ViewportGeometry.cursorInCrop(GPoint(0.25f - 5e-7f, 0.2f - 5e-7f), src),
        )
        assertPoint(
            GPoint(1f, 1f),
            ViewportGeometry.cursorInCrop(GPoint(0.75f + 5e-7f, 0.8f + 5e-7f), src),
        )
        assertNull(ViewportGeometry.cursorInCrop(GPoint(0.25f - 2e-6f, 0.5f), src))
        assertNull(ViewportGeometry.cursorInCrop(GPoint(0.75f + 2e-6f, 0.5f), src))
        assertNull(ViewportGeometry.cursorInCrop(GPoint(0.5f, 0.2f - 2e-6f), src))
        assertNull(ViewportGeometry.cursorInCrop(GPoint(0.5f, 0.8f + 2e-6f), src))
        assertPoint(
            GPoint(0.1f, 0.5f),
            ViewportGeometry.cursorInCrop(GPoint(0.1f, 0.5f), FULL.copy(x = -0.00005f)),
        )
    }

    private fun assertRect(expected: GRect, actual: GRect) {
        assertEquals(expected.x, actual.x, EPSILON)
        assertEquals(expected.y, actual.y, EPSILON)
        assertEquals(expected.w, actual.w, EPSILON)
        assertEquals(expected.h, actual.h, EPSILON)
    }

    private fun assertPoint(expected: GPoint, actual: GPoint?) {
        requireNotNull(actual)
        assertEquals(expected.x, actual.x, EPSILON)
        assertEquals(expected.y, actual.y, EPSILON)
    }

    private fun vp(zoom: Float, offsetX: Float, offsetY: Float): SessionViewport =
        SessionViewport(CONTENT, zoom, GPoint(offsetX, offsetY))

    companion object {
        private const val VIEW_W = 1000f
        private const val VIEW_H = 600f
        private val CONTENT = GRect(0f, 50f, 1000f, 500f)
        private val FULL = GRect(0f, 0f, 1f, 1f)
        private val NON_FINITE = listOf(Float.NaN, Float.NEGATIVE_INFINITY, Float.POSITIVE_INFINITY)
        private const val EPSILON = 1e-4f
    }
}
