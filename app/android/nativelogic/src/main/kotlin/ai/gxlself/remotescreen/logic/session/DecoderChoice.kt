package ai.gxlself.remotescreen.logic.session

data class CodecCandidate(
    val name: String,
    val hardware: Boolean,
    val lowLatencyFeature: Boolean,
    val softwareOnly: Boolean,
)

object DecoderChoice {
    fun choose(candidates: List<CodecCandidate>): CodecCandidate? =
        candidates.firstOrNull { it.hardware && it.lowLatencyFeature }
            ?: candidates.firstOrNull { it.hardware }
            ?: candidates.firstOrNull()

    fun formatKeys(codecName: String, sdk: Int): Map<String, Int> =
        buildMap {
            put("priority", 0)
            if (sdk >= 30) put("low-latency", 1)
            val name = codecName.lowercase()
            when {
                name.startsWith("c2.qti") || name.startsWith("omx.qcom") -> {
                    put("vendor.qti-ext-dec-low-latency.enable", 1)
                }
                name.startsWith("c2.hisi") || name.startsWith("omx.hisi") -> {
                    put("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1)
                    put("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy", -1)
                }
                name.startsWith("c2.rk") || name.startsWith("omx.rk") -> {
                    put("vendor.rtc-ext-dec-low-latency.enable", 1)
                }
                name.startsWith("c2.exynos") || name.startsWith("omx.exynos") -> {
                    put("vendor.low-latency.enable", 1)
                }
            }
        }
}
