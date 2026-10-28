package ai.gxlself.remotescreen.logic.gestures

class SessionViewport(
    var contentRect: GRect = GRect.ZERO,
    zoom: Float = MINIMUM_ZOOM,
    offset: GPoint = GPoint.ZERO,
) {
    var zoom: Float = sanitizeZoom(zoom)
        set(value) {
            field = sanitizeZoom(value)
            offset = offset
        }

    var offset: GPoint = clampOffset(offset, this.zoom)
        set(value) {
            field = clampOffset(value, zoom)
        }

    fun normalizedFromView(point: GPoint): GPoint {
        if (!hasContent()) return offset
        return GPoint(
            x = offset.x + clamp((point.x - contentRect.x) / contentRect.w) / zoom,
            y = offset.y + clamp((point.y - contentRect.y) / contentRect.h) / zoom,
        )
    }

    fun viewPointFromNormalized(point: GPoint): GPoint =
        GPoint(
            x = contentRect.x + (point.x - offset.x) * zoom * contentRect.w,
            y = contentRect.y + (point.y - offset.y) * zoom * contentRect.h,
        )

    fun normalizedDeltaFromViewDelta(delta: GPoint): GPoint {
        if (!hasContent() || !delta.x.isFinite() || !delta.y.isFinite()) return GPoint.ZERO
        return GPoint(
            x = delta.x / (contentRect.w * zoom),
            y = delta.y / (contentRect.h * zoom),
        )
    }

    private fun hasContent(): Boolean =
        contentRect.w.isFinite() &&
            contentRect.h.isFinite() &&
            contentRect.w > 0f &&
            contentRect.h > 0f

    companion object {
        const val MINIMUM_ZOOM = 1f
        const val MAXIMUM_ZOOM = 4f

        private fun sanitizeZoom(value: Float): Float =
            if (value.isFinite()) value.coerceIn(MINIMUM_ZOOM, MAXIMUM_ZOOM) else MINIMUM_ZOOM

        private fun clampOffset(point: GPoint, zoom: Float): GPoint {
            val limit = 1f - 1f / zoom
            return GPoint(clamp(point.x, limit), clamp(point.y, limit))
        }

        private fun clamp(value: Float, upper: Float = 1f): Float =
            if (value.isFinite()) value.coerceIn(0f, upper) else 0f
    }
}
