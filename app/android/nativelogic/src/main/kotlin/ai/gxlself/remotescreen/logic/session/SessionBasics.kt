package ai.gxlself.remotescreen.logic.session

enum class CodecDecision {
    HEVC,
    H264,
    NO_PARAMETER_SETS,
}

object SessionCodec {
    fun detect(vpsLen: Int, spsLen: Int, ppsLen: Int): CodecDecision =
        when {
            vpsLen > 0 && spsLen > 0 && ppsLen > 0 -> CodecDecision.HEVC
            vpsLen == 0 && spsLen > 0 && ppsLen > 0 -> CodecDecision.H264
            else -> CodecDecision.NO_PARAMETER_SETS
        }

    fun mime(codec: Int): String? =
        when (codec) {
            1 -> "video/hevc"
            2 -> "video/avc"
            else -> null
        }
}

sealed interface EpochChange {
    object First : EpochChange

    object Same : EpochChange

    data class Changed(val from: Int, val to: Int) : EpochChange
}

class EpochTracker {
    private var current: Int? = null

    fun observe(epoch: Int): EpochChange {
        val previous = current
        if (previous == null) {
            current = epoch
            return EpochChange.First
        }
        if (previous == epoch) return EpochChange.Same
        current = epoch
        return EpochChange.Changed(from = previous, to = epoch)
    }
}

data class RectF4(val x: Float, val y: Float, val w: Float, val h: Float)

object ContentRect {
    fun fit(videoW: Float, videoH: Float, boundsW: Float, boundsH: Float): RectF4 {
        if (videoW <= 0f || videoH <= 0f || boundsW <= 0f || boundsH <= 0f) {
            return RectF4(x = 0f, y = 0f, w = 0f, h = 0f)
        }
        val scale = minOf(boundsW / videoW, boundsH / videoH)
        val width = videoW * scale
        val height = videoH * scale
        return RectF4(
            x = (boundsW - width) / 2f,
            y = (boundsH - height) / 2f,
            w = width,
            h = height,
        )
    }
}
