package ai.gxlself.remotescreen.logic.viewport

import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import kotlin.math.abs
import kotlin.math.floor
import kotlin.math.roundToInt

data class PixelSize(val width: Int, val height: Int)

object ViewportGeometry {
    private const val FULL_TOLERANCE = 1e-4f
    private const val CURSOR_TOLERANCE = 1e-6f
    private val FULL = GRect(0f, 0f, 1f, 1f)

    /**
     * Returns the part of the normalized display that is visible in the view.
     * The view bounds are inverse-transformed without first clamping them to the
     * content rectangle, so letterbox space is accounted for before the result
     * is clipped to normalized display coordinates.
     */
    fun visibleRect(
        viewport: SessionViewport,
        viewW: Float,
        viewH: Float,
        keyboardInset: Float,
    ): GRect {
        val content = viewport.contentRect
        val zoom = viewport.zoom
        val offset = viewport.offset
        if (!viewW.isFinite() || viewW <= 0f ||
            !viewH.isFinite() || viewH <= 0f ||
            !content.isUsableContent() ||
            !zoom.isFinite() || zoom <= 0f ||
            !offset.x.isFinite() || !offset.y.isFinite()
        ) {
            return FULL
        }

        val inset = if (keyboardInset.isFinite()) {
            keyboardInset.coerceIn(0f, viewH)
        } else {
            0f
        }
        val bottom = viewH - inset
        val scaleX = zoom * content.w
        val scaleY = zoom * content.h
        if (!scaleX.isFinite() || scaleX <= 0f || !scaleY.isFinite() || scaleY <= 0f) {
            return FULL
        }

        val left = offset.x + (0f - content.x) / scaleX
        val right = offset.x + (viewW - content.x) / scaleX
        val top = offset.y + (0f - content.y) / scaleY
        val bottomNormalized = offset.y + (bottom - content.y) / scaleY
        if (!left.isFinite() || !right.isFinite() ||
            !top.isFinite() || !bottomNormalized.isFinite()
        ) {
            return FULL
        }

        val x0 = clamp01(left)
        val x1 = clamp01(right)
        val y0 = clamp01(top)
        val y1 = clamp01(bottomNormalized)
        return GRect(
            x = x0,
            y = y0,
            w = x1 - x0,
            h = y1 - y0,
        )
    }

    /** Returns floor(visible size * zoom * content size), without a density multiplier. */
    fun viewPixels(visible: GRect, viewport: SessionViewport): PixelSize =
        if (!visible.isFiniteRect() || !viewport.contentRect.isFiniteRect()) {
            PixelSize(0, 0)
        } else {
            PixelSize(
                width = pixels(visible.w, viewport.zoom, viewport.contentRect.w),
                height = pixels(visible.h, viewport.zoom, viewport.contentRect.h),
            )
        }

    /** Sanitizes an event source rectangle into normalized display coordinates. */
    fun source(x: Float, y: Float, w: Float, h: Float): GRect {
        if (!x.isFinite() || !y.isFinite() || !w.isFinite() || !h.isFinite() ||
            w <= 0f || h <= 0f
        ) {
            return FULL
        }
        val sourceX = clamp01(x)
        val sourceY = clamp01(y)
        return GRect(
            x = sourceX,
            y = sourceY,
            w = minOf(w, 1f - sourceX),
            h = minOf(h, 1f - sourceY),
        )
    }

    /** Returns whether a source rectangle is the complete display, within tolerance. */
    fun isFull(src: GRect): Boolean =
        abs(src.x) <= FULL_TOLERANCE &&
            abs(src.y) <= FULL_TOLERANCE &&
            abs(src.w - 1f) <= FULL_TOLERANCE &&
            abs(src.h - 1f) <= FULL_TOLERANCE

    /** Maps a normalized source crop into the view's content rectangle. */
    fun cropFrame(contentRect: GRect, src: GRect): GRect {
        if (!contentRect.isFiniteRect() || contentRect.w < 0f || contentRect.h < 0f) {
            return GRect.ZERO
        }
        if (!src.isValidSource()) return GRect.ZERO

        val result = GRect(
            x = contentRect.x + src.x * contentRect.w,
            y = contentRect.y + src.y * contentRect.h,
            w = src.w * contentRect.w,
            h = src.h * contentRect.h,
        )
        return if (result.isFiniteRect()) result else GRect.ZERO
    }

    /** Computes the logical full-screen dimensions represented by a decoded crop. */
    fun logicalVideoSize(decodedW: Int, decodedH: Int, src: GRect): PixelSize {
        val decoded = PixelSize(decodedW, decodedH)
        if (decodedW <= 0 || decodedH <= 0 || isFull(src) || !src.isValidSource()) {
            return decoded
        }

        val widthScale = decodedW.toFloat() / src.w
        val heightScale = decodedH.toFloat() / src.h
        if (!widthScale.isFinite() || !heightScale.isFinite()) return decoded
        return PixelSize(widthScale.roundToInt(), heightScale.roundToInt())
    }

    /** Maps a full-screen normalized cursor into a normalized crop cursor. */
    fun cursorInCrop(cursor: GPoint, src: GRect): GPoint? {
        if (!cursor.x.isFinite() || !cursor.y.isFinite()) return null
        if (isFull(src)) return cursor
        if (!src.isValidSource()) return null

        val maxX = src.x + src.w
        val maxY = src.y + src.h
        if (cursor.x < src.x - CURSOR_TOLERANCE || cursor.x > maxX + CURSOR_TOLERANCE ||
            cursor.y < src.y - CURSOR_TOLERANCE || cursor.y > maxY + CURSOR_TOLERANCE
        ) {
            return null
        }

        return GPoint(
            x = ((cursor.x - src.x) / src.w).coerceIn(0f, 1f),
            y = ((cursor.y - src.y) / src.h).coerceIn(0f, 1f),
        )
    }

    private fun pixels(dimension: Float, zoom: Float, contentDimension: Float): Int {
        if (!dimension.isFinite() || dimension < 0f ||
            !zoom.isFinite() || zoom < 0f ||
            !contentDimension.isFinite() || contentDimension < 0f
        ) {
            return 0
        }
        val value = dimension * zoom * contentDimension
        return if (!value.isFinite() || value < 0f) 0 else floor(value).toInt()
    }

    private fun clamp01(value: Float): Float = value.coerceIn(0f, 1f)

    private fun GRect.isUsableContent(): Boolean =
        isFiniteRect() && w > 0f && h > 0f

    private fun GRect.isFiniteRect(): Boolean =
        x.isFinite() && y.isFinite() && w.isFinite() && h.isFinite() &&
            (x + w).isFinite() && (y + h).isFinite()

    private fun GRect.isNormalizedRect(): Boolean =
        isFiniteRect() &&
            w >= 0f && h >= 0f &&
            x in 0f..1f && y in 0f..1f &&
            x + w <= 1f && y + h <= 1f

    private fun GRect.isValidSource(): Boolean =
        isNormalizedRect() && w > 0f && h > 0f
}
