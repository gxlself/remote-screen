package ai.gxlself.remotescreen.logic.gestures

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MultiFingerTapsTest {
    @Test
    fun twoFingerTapWaitsForDoubleTapWindow() {
        val taps = MultiFingerTaps()
        assertTrue(twoFingerTap(taps, 0).isEmpty())
        assertTrue(taps.tick(349).isEmpty())
        assertEquals(listOf(TapResult.TwoFingerTap(15f, 25f)), taps.tick(350))
    }

    @Test
    fun twoFingerDoubleTapOnlyEmitsDoubleTap() {
        val taps = MultiFingerTaps()
        val results = mutableListOf<TapResult>()
        results += twoFingerTap(taps, 0)
        results += twoFingerTap(taps, 200)
        results += taps.tick(1_000)
        assertEquals(listOf(TapResult.TwoFingerDoubleTap), results)
    }

    @Test
    fun doubleTapMayStartAtWindowBoundary() {
        val taps = MultiFingerTaps()
        twoFingerTap(taps, 0)
        val results = twoFingerTap(taps, 350)
        assertEquals(listOf(TapResult.TwoFingerDoubleTap), results)
    }

    @Test
    fun tickDoesNotResolvePendingTapDuringPossibleSecondTap() {
        val taps = MultiFingerTaps()
        twoFingerTap(taps, 0)
        taps.down(1, 10f, 20f, 300)
        assertTrue(taps.tick(350).isEmpty())
        taps.down(2, 20f, 30f, 400)
        taps.up(1, 500)
        assertEquals(listOf(TapResult.TwoFingerDoubleTap), taps.up(2, 550))
    }

    @Test
    fun threeFingerTapEmitsImmediately() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 1f, 1f, 10)
        taps.down(3, 2f, 2f, 20)
        taps.up(1, 30)
        taps.up(2, 40)
        assertEquals(listOf(TapResult.ThreeFingerTap), taps.up(3, 50))
    }

    @Test
    fun tapOverMaximumDurationIsIgnored() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 1f, 1f, 1)
        taps.up(1, 100)
        assertTrue(taps.up(2, 301).isEmpty())
        assertTrue(taps.tick(1_000).isEmpty())
    }

    @Test
    fun movementBeyondSlopIsIgnored() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 10f, 10f, 1)
        taps.move(1, 10.01f, 0f, 10)
        taps.up(1, 20)
        assertTrue(taps.up(2, 30).isEmpty())
        assertTrue(taps.tick(1_000).isEmpty())
    }

    @Test
    fun staggeredFingerDownsStillFormTap() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 20f, 40f, 100)
        taps.up(1, 200)
        assertTrue(taps.up(2, 300).isEmpty())
        assertEquals(listOf(TapResult.TwoFingerTap(10f, 20f)), taps.tick(600))
    }

    @Test
    fun fourFingerTapIsIgnored() {
        val taps = MultiFingerTaps()
        repeat(4) { taps.down(it, it.toFloat(), 0f, it.toLong()) }
        repeat(3) { taps.up(it, 10L + it) }
        assertTrue(taps.up(3, 20).isEmpty())
    }

    @Test
    fun singleFingerTapIsIgnoredAndIsNotMultiFinger() {
        val taps = MultiFingerTaps()
        assertTrue(taps.down(1, 0f, 0f, 0).isEmpty())
        assertFalse(taps.isMultiFinger)
        assertTrue(taps.up(1, 10).isEmpty())
        assertFalse(taps.isMultiFinger)
    }

    @Test
    fun pendingTapEmitsWhenNewSequenceEndsAsSingleFinger() {
        val taps = MultiFingerTaps()
        twoFingerTap(taps, 0)
        assertTrue(taps.down(3, 0f, 0f, 100).isEmpty())
        assertEquals(listOf(TapResult.TwoFingerTap(15f, 25f)), taps.up(3, 110))
    }

    @Test
    fun thirdFingerDownFlushesPendingTap() {
        val taps = MultiFingerTaps()
        twoFingerTap(taps, 0)
        taps.down(3, 0f, 0f, 100)
        taps.down(4, 1f, 1f, 101)
        assertEquals(listOf(TapResult.TwoFingerTap(15f, 25f)), taps.down(5, 2f, 2f, 102))
    }

    @Test
    fun cancelDropsCurrentSequenceButKeepsPendingTap() {
        val taps = MultiFingerTaps()
        twoFingerTap(taps, 0)
        taps.down(3, 0f, 0f, 100)
        taps.down(4, 1f, 1f, 101)
        taps.cancel()
        assertFalse(taps.isMultiFinger)
        assertTrue(taps.up(3, 120).isEmpty())
        assertEquals(listOf(TapResult.TwoFingerTap(15f, 25f)), taps.tick(350))
    }

    @Test
    fun cancelPreventsCancelledSequenceOutput() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 1f, 1f, 1)
        taps.cancel()
        assertTrue(taps.up(1, 2).isEmpty())
        assertTrue(taps.up(2, 3).isEmpty())
        assertTrue(taps.tick(1_000).isEmpty())
    }

    @Test
    fun nonFiniteCoordinateInvalidatesSequence() {
        val taps = MultiFingerTaps()
        taps.down(1, Float.NaN, 0f, 0)
        taps.down(2, 1f, 1f, 1)
        taps.up(1, 2)
        assertTrue(taps.up(2, 3).isEmpty())
    }

    @Test
    fun movementAtExactlySlopIsAccepted() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 20f, 40f, 1)
        taps.move(1, 10f, 0f, 2)
        taps.up(1, 3)
        taps.up(2, 4)
        assertEquals(listOf(TapResult.TwoFingerTap(10f, 20f)), taps.tick(304))
    }

    @Test
    fun multiFingerFlagStaysTrueThroughSequenceEnd() {
        val taps = MultiFingerTaps()
        taps.down(1, 0f, 0f, 0)
        taps.down(2, 1f, 1f, 1)
        assertTrue(taps.isMultiFinger)
        taps.up(1, 2)
        taps.up(2, 3)
        assertTrue(taps.isMultiFinger)
        taps.down(3, 0f, 0f, 10)
        assertFalse(taps.isMultiFinger)
    }

    private fun twoFingerTap(taps: MultiFingerTaps, startMs: Long): List<TapResult> {
        val results = mutableListOf<TapResult>()
        results += taps.down(1, 10f, 20f, startMs)
        results += taps.down(2, 20f, 30f, startMs + 10)
        results += taps.up(1, startMs + 40)
        results += taps.up(2, startMs + 50)
        return results
    }
}
