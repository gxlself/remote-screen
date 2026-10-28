package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class DecodeStatsTest {
    @Test
    fun emptyStatsHaveExpectedDefaults() {
        val snapshot = DecodeStats().snapshot(0, 2, 3, 4, 1920, 1080, 0)
        assertEquals(0, snapshot.fps)
        assertEquals(0f, snapshot.decodeP50Ms)
        assertEquals(0f, snapshot.decodeP95Ms)
        assertEquals(-1f, snapshot.latencyP50Ms)
        assertEquals("", snapshot.codec)
        assertEquals(2, snapshot.queue)
    }

    @Test
    fun calculatesNearestRankDecodePercentiles() {
        val stats = DecodeStats()
        for (millisecond in 1L..120L) stats.recordDecoded(millisecond * 1_000, null, millisecond)
        val snapshot = stats.snapshot(120, 0, 0, 0, 0, 0, 1)
        assertEquals(60f, snapshot.decodeP50Ms)
        assertEquals(114f, snapshot.decodeP95Ms)
        assertEquals("hevc", snapshot.codec)
    }

    @Test
    fun decodePercentilesOnlyUseMostRecentWindow() {
        val stats = DecodeStats()
        for (millisecond in 1L..240L) stats.recordDecoded(millisecond * 1_000, null, millisecond)
        val snapshot = stats.snapshot(240, 0, 0, 0, 0, 0, 2)
        assertEquals(180f, snapshot.decodeP50Ms)
        assertEquals(234f, snapshot.decodeP95Ms)
        assertEquals("h264", snapshot.codec)
    }

    @Test
    fun latencyHasIndependentWindowAndNearestRankMedian() {
        val stats = DecodeStats(window = 3)
        stats.recordDecoded(1, 30_000, 0)
        stats.recordDecoded(1, null, 1)
        stats.recordDecoded(1, 10_000, 2)
        stats.recordDecoded(1, 20_000, 3)
        assertEquals(20f, stats.snapshot(3, 0, 0, 0, 0, 0, 0).latencyP50Ms)
    }

    @Test
    fun millisecondsAreRoundedToTwoDecimalPlaces() {
        val stats = DecodeStats()
        stats.recordDecoded(1_236, 2_344, 0)
        val snapshot = stats.snapshot(0, 0, 0, 0, 0, 0, 0)
        assertEquals(1.24f, snapshot.decodeP50Ms)
        assertEquals(2.34f, snapshot.latencyP50Ms)
    }

    @Test
    fun fpsExcludesFrameExactlyOneSecondOld() {
        val stats = DecodeStats()
        stats.recordDecoded(1, null, 0)
        stats.recordDecoded(1, null, 1)
        stats.recordDecoded(1, null, 999)
        stats.recordDecoded(1, null, 1_000)
        assertEquals(3, stats.snapshot(1_000, 0, 0, 0, 0, 0, 0).fps)
    }

    @Test
    fun fpsDoesNotCountFutureFrame() {
        val stats = DecodeStats()
        stats.recordDecoded(1, null, 101)
        assertEquals(0, stats.snapshot(100, 0, 0, 0, 0, 0, 0).fps)
    }

    @Test
    fun remembersLastReportValuesAndClearResetsEverything() {
        val stats = DecodeStats()
        stats.recordDecoded(12, 34, 50)
        assertEquals(12, stats.lastDecodeUs())
        assertEquals(34, stats.lastLatencyUs())
        stats.recordDecoded(56, null, 51)
        assertEquals(56, stats.lastDecodeUs())
        assertEquals(0, stats.lastLatencyUs())
        stats.clear()
        assertEquals(0, stats.lastDecodeUs())
        assertEquals(0, stats.lastLatencyUs())
        assertEquals(0, stats.snapshot(50, 0, 0, 0, 0, 0, 0).fps)
    }

    @Test
    fun latencyRequiresClockSynchronization() {
        assertNull(Latency.latencyUs(clockSynced = false, captureLocalUs = 100, nowUs = 200))
    }

    @Test
    fun latencyRejectsNegativeResult() {
        assertNull(Latency.latencyUs(clockSynced = true, captureLocalUs = 201, nowUs = 200))
    }

    @Test
    fun latencyReturnsNonNegativeDifference() {
        assertEquals(100L, Latency.latencyUs(clockSynced = true, captureLocalUs = 100, nowUs = 200))
        assertEquals(0L, Latency.latencyUs(clockSynced = true, captureLocalUs = 200, nowUs = 200))
    }
}
