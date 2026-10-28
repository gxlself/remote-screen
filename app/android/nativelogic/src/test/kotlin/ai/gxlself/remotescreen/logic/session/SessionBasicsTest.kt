package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SessionBasicsTest {
    @Test
    fun detectsHevc() {
        assertEquals(CodecDecision.HEVC, SessionCodec.detect(vpsLen = 1, spsLen = 1, ppsLen = 1))
    }

    @Test
    fun detectsH264() {
        assertEquals(CodecDecision.H264, SessionCodec.detect(vpsLen = 0, spsLen = 1, ppsLen = 1))
    }

    @Test
    fun detectsMissingParameterSets() {
        assertEquals(CodecDecision.NO_PARAMETER_SETS, SessionCodec.detect(1, 0, 1))
        assertEquals(CodecDecision.NO_PARAMETER_SETS, SessionCodec.detect(0, 0, 0))
        assertEquals(CodecDecision.NO_PARAMETER_SETS, SessionCodec.detect(-1, 1, 1))
    }

    @Test
    fun mapsCodecMimeTypes() {
        assertEquals("video/hevc", SessionCodec.mime(1))
        assertEquals("video/avc", SessionCodec.mime(2))
        assertNull(SessionCodec.mime(0))
    }

    @Test
    fun tracksEpochChanges() {
        val tracker = EpochTracker()
        assertEquals(EpochChange.First, tracker.observe(3))
        assertEquals(EpochChange.Same, tracker.observe(3))
        assertEquals(EpochChange.Changed(from = 3, to = 4), tracker.observe(4))
        assertEquals(EpochChange.Same, tracker.observe(4))
    }

    @Test
    fun fitsLandscapeVideo() {
        val rect = ContentRect.fit(videoW = 1920f, videoH = 1080f, boundsW = 2556f, boundsH = 1179f)
        assertEquals(2096f, rect.w, 1f)
        assertEquals(230f, rect.x, 1f)
        assertEquals(0f, rect.y, 0.01f)
    }

    @Test
    fun fitsLandscapeVideoInPortraitBounds() {
        val rect = ContentRect.fit(videoW = 1920f, videoH = 1080f, boundsW = 1179f, boundsH = 2556f)
        assertEquals(663.1875f, rect.h, 1f)
        assertEquals(1278f, rect.y + rect.h / 2f, 1f)
    }

    @Test
    fun invalidContentDimensionsReturnZeroRect() {
        val zero = RectF4(0f, 0f, 0f, 0f)
        assertEquals(zero, ContentRect.fit(0f, 1080f, 1f, 1f))
        assertEquals(zero, ContentRect.fit(1f, 1f, -1f, 1f))
    }
}
