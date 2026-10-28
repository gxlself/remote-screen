package ai.gxlself.remotescreen.logic.session

import kotlin.math.ceil
import kotlin.math.round

data class VideoStatsSnapshot(
    val fps: Int,
    val decodeP50Ms: Float,
    val decodeP95Ms: Float,
    val latencyP50Ms: Float,
    val queue: Int,
    val dropped: Int,
    val keyframeRequests: Int,
    val width: Int,
    val height: Int,
    val codec: String,
)

class DecodeStats(private val window: Int = 120) {
    private val decodeSamples = ArrayDeque<Long>()
    private val latencySamples = ArrayDeque<Long>()
    private val displayedAt = ArrayDeque<Long>()
    private var latestDecodeUs = 0L
    private var latestLatencyUs = 0L

    init {
        require(window > 0) { "window must be positive" }
    }

    fun recordDecoded(decodeUs: Long, latencyUs: Long?, displayedAtMs: Long) {
        addBounded(decodeSamples, decodeUs)
        latestDecodeUs = decodeUs
        if (latencyUs != null && latencyUs >= 0) {
            addBounded(latencySamples, latencyUs)
            latestLatencyUs = latencyUs
        } else {
            latestLatencyUs = 0
        }
        displayedAt.addLast(displayedAtMs)
    }

    fun snapshot(
        nowMs: Long,
        queue: Int,
        dropped: Int,
        keyframeRequests: Int,
        width: Int,
        height: Int,
        codec: Int,
    ): VideoStatsSnapshot {
        val cutoff = nowMs - 1_000L
        while (displayedAt.isNotEmpty() && displayedAt.first() <= cutoff) {
            displayedAt.removeFirst()
        }
        val fps = displayedAt.count { it <= nowMs }
        return VideoStatsSnapshot(
            fps = fps,
            decodeP50Ms = percentileMs(decodeSamples, 0.50),
            decodeP95Ms = percentileMs(decodeSamples, 0.95),
            latencyP50Ms = if (latencySamples.isEmpty()) -1f else percentileMs(latencySamples, 0.50),
            queue = queue,
            dropped = dropped,
            keyframeRequests = keyframeRequests,
            width = width,
            height = height,
            codec =
                when (codec) {
                    1 -> "hevc"
                    2 -> "h264"
                    else -> ""
                },
        )
    }

    fun lastDecodeUs(): Long = latestDecodeUs

    fun lastLatencyUs(): Long = latestLatencyUs

    fun clear() {
        decodeSamples.clear()
        latencySamples.clear()
        displayedAt.clear()
        latestDecodeUs = 0
        latestLatencyUs = 0
    }

    private fun addBounded(samples: ArrayDeque<Long>, value: Long) {
        samples.addLast(value)
        if (samples.size > window) samples.removeFirst()
    }

    private fun percentileMs(samples: Collection<Long>, percentile: Double): Float {
        if (samples.isEmpty()) return 0f
        val sorted = samples.sorted()
        val rank = ceil(percentile * sorted.size).toInt().coerceAtLeast(1)
        val milliseconds = sorted[rank - 1] / 1_000f
        return round(milliseconds * 100f) / 100f
    }
}

object Latency {
    fun latencyUs(clockSynced: Boolean, captureLocalUs: Long, nowUs: Long): Long? {
        if (!clockSynced || nowUs < captureLocalUs) return null
        return nowUs - captureLocalUs
    }
}
