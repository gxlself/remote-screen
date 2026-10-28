package ai.gxlself.remotescreen.logic.zoom

import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import kotlin.math.max

/** A small, platform-free viewport value used by zoom calculations and tests. */
data class Viewport(
    val zoom: Float = 1f,
    val offset: GPoint = GPoint.ZERO,
)

/** Pure geometry for the local remote-screen viewport. */
object ZoomMath {
    const val MIN_ZOOM = 1f
    const val MAX_ZOOM = 4f

    fun clampOffset(zoom: Float, offset: GPoint): GPoint {
        val safeZoom = sanitizeZoom(zoom)
        val limit = max(0f, 1f - 1f / safeZoom)
        return GPoint(
            clamp(offset.x, limit),
            clamp(offset.y, limit),
        )
    }

    fun zoomed(vp: Viewport, factor: Float, anchorNorm: GPoint): Viewport {
        val oldZoom = sanitizeZoom(vp.zoom)
        val scale = factor.takeIf { it.isFinite() && it > 0f } ?: 1f
        val newZoom = (oldZoom * scale).takeIf { it.isFinite() }?.coerceIn(MIN_ZOOM, MAX_ZOOM)
            ?: oldZoom
        val anchor = normalized(anchorNorm)
        val nextOffset = GPoint(
            anchor.x - (anchor.x - clampOffset(oldZoom, vp.offset).x) * oldZoom / newZoom,
            anchor.y - (anchor.y - clampOffset(oldZoom, vp.offset).y) * oldZoom / newZoom,
        )
        return Viewport(newZoom, clampOffset(newZoom, nextOffset))
    }

    fun panned(vp: Viewport, deltaNorm: GPoint): Viewport {
        val zoom = sanitizeZoom(vp.zoom)
        val offset = clampOffset(zoom, vp.offset)
        val delta = GPoint(
            deltaNorm.x.takeIf { it.isFinite() } ?: 0f,
            deltaNorm.y.takeIf { it.isFinite() } ?: 0f,
        )
        return Viewport(
            zoom,
            clampOffset(zoom, GPoint(offset.x - delta.x, offset.y - delta.y)),
        )
    }

    fun followCursor(vp: Viewport, cursor: GPoint, margin: Float = 0.1f): Viewport {
        val zoom = sanitizeZoom(vp.zoom)
        val current = clampOffset(zoom, vp.offset)
        val point = normalized(cursor)
        val safeMargin = margin.takeIf { it.isFinite() }?.coerceIn(0f, 0.5f) ?: 0.1f
        val visible = 1f / zoom
        val low = current.x
        val high = current.x + visible
        val leftEdge = low + visible * safeMargin
        val rightEdge = high - visible * safeMargin
        val nextX = when {
            point.x < leftEdge -> point.x - visible * safeMargin
            point.x > rightEdge -> point.x - visible * (1f - safeMargin)
            else -> current.x
        }
        val top = current.y
        val bottom = current.y + visible
        val topEdge = top + visible * safeMargin
        val bottomEdge = bottom - visible * safeMargin
        val nextY = when {
            point.y < topEdge -> point.y - visible * safeMargin
            point.y > bottomEdge -> point.y - visible * (1f - safeMargin)
            else -> current.y
        }
        return Viewport(zoom, clampOffset(zoom, GPoint(nextX, nextY)))
    }

    fun keyboardTarget(
        vp: Viewport,
        cursor: GPoint,
        insetFraction: Float,
        minZoom: Float = 1.7f,
    ): Viewport {
        val targetZoom = sanitizeZoom(max(sanitizeZoom(vp.zoom), minZoom))
        val inset = insetFraction.takeIf { it.isFinite() }?.coerceIn(0f, 0.99f) ?: 0f
        val point = normalized(cursor)
        val visible = 1f / targetZoom
        val availableHeight = visible * (1f - inset)
        val target = GPoint(
            point.x - visible / 2f,
            point.y - availableHeight / 2f,
        )
        return Viewport(targetZoom, clampOffset(targetZoom, target))
    }

    /** Adapts the platform-free value to the mutable R4A viewport. */
    fun toSessionViewport(value: Viewport, contentRect: GRect = GRect.ZERO): SessionViewport =
        SessionViewport(contentRect = contentRect, zoom = value.zoom, offset = value.offset)

    private fun sanitizeZoom(value: Float): Float =
        if (value.isFinite()) value.coerceIn(MIN_ZOOM, MAX_ZOOM) else MIN_ZOOM

    private fun normalized(point: GPoint): GPoint =
        GPoint(
            point.x.takeIf { it.isFinite() }?.coerceIn(0f, 1f) ?: 0f,
            point.y.takeIf { it.isFinite() }?.coerceIn(0f, 1f) ?: 0f,
        )

    private fun clamp(value: Float, upper: Float): Float =
        value.takeIf { it.isFinite() }?.coerceIn(0f, upper) ?: 0f
}

/** Maps a point in the Android content rectangle to remote normalized coordinates. */
object TouchTapMapper {
    fun normalized(viewPointDp: GPoint, contentRectDp: GRect, vp: Viewport): GPoint {
        val w = contentRectDp.w
        val h = contentRectDp.h
        val x = if (w.isFinite() && w > 0f) {
            ((viewPointDp.x - contentRectDp.x) / w).coerceIn(0f, 1f)
        } else 0f
        val y = if (h.isFinite() && h > 0f) {
            ((viewPointDp.y - contentRectDp.y) / h).coerceIn(0f, 1f)
        } else 0f
        val zoom = vp.zoom.takeIf { it.isFinite() && it > 0f }?.coerceIn(1f, ZoomMath.MAX_ZOOM)
            ?: 1f
        val offset = ZoomMath.clampOffset(zoom, vp.offset)
        return GPoint(
            (offset.x + x / zoom).coerceIn(0f, 1f),
            (offset.y + y / zoom).coerceIn(0f, 1f),
        )
    }

    fun normalized(viewPointDp: GPoint, contentRectDp: GRect, vp: SessionViewport): GPoint =
        normalized(viewPointDp, contentRectDp, Viewport(vp.zoom, vp.offset))
}

object MagnifierPlacement {
    fun center(
        fingerDp: GPoint,
        viewHeightDp: Float,
        diameterDp: Float = 120f,
        offsetDp: Float = 80f,
    ): GPoint {
        val diameter = diameterDp.takeIf { it.isFinite() && it > 0f } ?: 120f
        val offset = offsetDp.takeIf { it.isFinite() && it >= 0f } ?: 80f
        val above = fingerDp.y - offset
        val candidate = if (above - diameter / 2f < 0f) fingerDp.y + offset else above
        val height = viewHeightDp.takeIf { it.isFinite() && it > 0f }
        val y = if (height == null) candidate else {
            candidate.coerceIn(diameter / 2f, (height - diameter / 2f).coerceAtLeast(diameter / 2f))
        }
        return GPoint(fingerDp.x, y)
    }
}

/** Coalesces viewport callbacks to at most ten per second. */
class ViewportEmitter(private val minIntervalMs: Long = 100) {
    private var lastEmitted: Viewport? = null
    private var lastEmissionMs: Long? = null
    private var pending: Viewport? = null
    private var pendingAtMs: Long? = null

    fun offer(vp: Viewport, nowMs: Long): Viewport? {
        val value = Viewport(
            vp.zoom.takeIf { it.isFinite() }?.coerceIn(ZoomMath.MIN_ZOOM, ZoomMath.MAX_ZOOM)
                ?: ZoomMath.MIN_ZOOM,
            ZoomMath.clampOffset(vp.zoom, vp.offset),
        )
        val previous = lastEmitted
        if (previous != null && previous == value) {
            pending = null
            pendingAtMs = null
            return null
        }
        pending = value
        pendingAtMs = nowMs
        val last = lastEmissionMs
        if (last != null && nowMs < last + minIntervalMs.coerceAtLeast(0L)) return null
        pending = null
        pendingAtMs = null
        lastEmitted = value
        lastEmissionMs = nowMs
        return value
    }

    fun flush(): Viewport? {
        val value = pending ?: return null
        pending = null
        val emittedAt = pendingAtMs
        pendingAtMs = null
        if (lastEmitted == value) return null
        lastEmitted = value
        if (emittedAt != null) lastEmissionMs = emittedAt
        return value
    }
}
