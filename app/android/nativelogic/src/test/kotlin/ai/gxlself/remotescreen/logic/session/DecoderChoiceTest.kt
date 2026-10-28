package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

class DecoderChoiceTest {
    @Test
    fun hardwareDecoderIsPreferred() {
        val software = candidate("software", hardware = false, lowLatency = true, softwareOnly = true)
        val hardware = candidate("hardware", hardware = true)
        assertSame(hardware, DecoderChoice.choose(listOf(software, hardware)))
    }

    @Test
    fun lowLatencyHardwareDecoderIsPreferred() {
        val regular = candidate("regular", hardware = true)
        val lowLatency = candidate("low", hardware = true, lowLatency = true)
        assertSame(lowLatency, DecoderChoice.choose(listOf(regular, lowLatency)))
    }

    @Test
    fun originalOrderBreaksDecoderTies() {
        val first = candidate("first", hardware = true, lowLatency = true)
        val second = candidate("second", hardware = true, lowLatency = true)
        assertSame(first, DecoderChoice.choose(listOf(first, second)))
    }

    @Test
    fun firstDecoderIsReturnedWhenAllAreSoftware() {
        val first = candidate("first", hardware = false, softwareOnly = true)
        val second = candidate("second", hardware = false, lowLatency = true, softwareOnly = true)
        assertSame(first, DecoderChoice.choose(listOf(first, second)))
    }

    @Test
    fun emptyDecoderListReturnsNull() {
        assertNull(DecoderChoice.choose(emptyList()))
    }

    @Test
    fun sdk29OnlyHasPriorityForGenericDecoder() {
        assertEquals(mapOf("priority" to 0), DecoderChoice.formatKeys("c2.android.avc.decoder", 29))
    }

    @Test
    fun sdk30AddsStandardLowLatencyKey() {
        assertEquals(
            mapOf("priority" to 0, "low-latency" to 1),
            DecoderChoice.formatKeys("c2.android.avc.decoder", 30),
        )
    }

    @Test
    fun addsQtiKeysCaseInsensitively() {
        val expected = 1
        assertEquals(expected, DecoderChoice.formatKeys("C2.QTI.hevc.decoder", 29)[QTI])
        assertEquals(expected, DecoderChoice.formatKeys("OMX.qcom.video.decoder.avc", 29)[QTI])
    }

    @Test
    fun addsHisiKeys() {
        val keys = DecoderChoice.formatKeys("c2.hisi.decoder", 29)
        assertEquals(1, keys[HISI_REQUEST])
        assertEquals(-1, keys[HISI_READY])
        assertEquals(1, DecoderChoice.formatKeys("omx.hisi.decoder", 29)[HISI_REQUEST])
    }

    @Test
    fun addsRockchipKeys() {
        assertEquals(1, DecoderChoice.formatKeys("c2.rk.avc.decoder", 29)[ROCKCHIP])
        assertEquals(1, DecoderChoice.formatKeys("omx.rk.video_decoder", 29)[ROCKCHIP])
    }

    @Test
    fun addsExynosKeys() {
        assertEquals(1, DecoderChoice.formatKeys("c2.exynos.avc.decoder", 29)[EXYNOS])
        assertEquals(1, DecoderChoice.formatKeys("omx.exynos.avc.dec", 29)[EXYNOS])
    }

    private fun candidate(
        name: String,
        hardware: Boolean,
        lowLatency: Boolean = false,
        softwareOnly: Boolean = false,
    ) = CodecCandidate(name, hardware, lowLatency, softwareOnly)

    private companion object {
        const val QTI = "vendor.qti-ext-dec-low-latency.enable"
        const val HISI_REQUEST =
            "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req"
        const val HISI_READY =
            "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy"
        const val ROCKCHIP = "vendor.rtc-ext-dec-low-latency.enable"
        const val EXYNOS = "vendor.low-latency.enable"
    }
}
