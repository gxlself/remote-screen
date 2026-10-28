package ai.gxlself.remotescreen.logic.gestures

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class GestureTests {
    @Test
    fun acceleration() {
        assertEquals(1f, TrackpadAccel.gain(0f), EPSILON)
        assertEquals(1.5f, TrackpadAccel.gain(450f), EPSILON)
        assertEquals(3f, TrackpadAccel.gain(1_800f), EPSILON)
        assertEquals(3f, TrackpadAccel.gain(4_000f), EPSILON)
    }

    @Test
    fun longPressDragUsesTheSamePointerConversionAsSingleFingerMotion() {
        val deltas = listOf(
            GPoint(2f, -1f),
            GPoint(-4f, 3f),
            GPoint(1.5f, 2.5f),
        )
        val viewport = SessionViewport(contentRect = GRect(0f, 0f, 1_000f, 500f))
        val dragCursor = CursorModel()
        val drag = TrackpadDragState()
        assertTrue(drag.start())
        val expectedPositions = listOf(
            GPoint(0.5037714f, 0.4962286f),
            GPoint(0.4962286f, 0.5075429f),
            GPoint(0.49905714f, 0.5169714f),
        )
        deltas.forEachIndexed { index, delta ->
            val ordinaryMotion = TrackpadPointerMotion.calculate(
                delta,
                speedDpPerSecond = 420f,
                cursorSpeed = 1.8f,
            )
            val dragMotion = TrackpadPointerMotion.calculate(
                delta,
                speedDpPerSecond = 420f,
                cursorSpeed = 1.8f,
            )
            assertEquals(ordinaryMotion, dragMotion)
            dragCursor.apply(dragMotion.delta, viewport, dragMotion.gain)
            assertPoint(expectedPositions[index], dragCursor.position)
        }
        assertPoint(GPoint(0.49905714f, 0.5169714f), dragCursor.position)
        var buttonReleaseEvents = 0
        if (drag.release()) buttonReleaseEvents += 1
        if (drag.release()) buttonReleaseEvents += 1
        assertEquals(1, buttonReleaseEvents)
    }

    @Test
    fun secondFingerEndsDragAndDoesNotReleaseTwice() {
        val drag = TrackpadDragState()
        assertTrue(drag.start())
        assertTrue(drag.cancelForMultiFinger())
        assertFalse(drag.isDragging)
        assertFalse(drag.cancelForMultiFinger())
    }

    @Test
    fun closeOnceRunsCleanupOnlyOnce() {
        var releases = 0
        val close = CloseOnce { releases += 1 }
        assertTrue(close.close())
        assertFalse(close.close())
        assertTrue(close.isClosed)
        assertEquals(1, releases)
    }

    @Test
    fun cursorMovementAndClamping() {
        val cursor = CursorModel()
        val viewport = SessionViewport(contentRect = GRect(0f, 0f, 1_000f, 500f))
        cursor.apply(GPoint(100f, 50f), viewport, 1f)
        assertPoint(GPoint(0.6f, 0.6f), cursor.position)
        cursor.apply(GPoint(-2_000f, 1_000f), viewport, 3f)
        assertEquals(GPoint(0f, 1f), cursor.position)
        cursor.apply(GPoint(20f, 20f), SessionViewport(), 1f)
        assertEquals(GPoint(0f, 1f), cursor.position)
    }

    @Test
    fun clickCountAndReset() {
        val clicks = ClickCounter()
        val point = GPoint(0.5f, 0.5f)
        assertEquals(1, clicks.register(point, 0.0))
        assertEquals(2, clicks.register(point, 0.1))
        assertEquals(3, clicks.register(point, 0.2))
        assertEquals(3, clicks.register(point, 0.3))
        assertEquals(1, clicks.register(point, 0.8))
        assertEquals(1, clicks.register(GPoint(0.52f, 0.5f), 0.9))
    }

    @Test
    fun momentumDecaysAndEnds() {
        val momentum = ScrollMomentum()
        momentum.start(GPoint(600f, 0f))
        var previous = 600f
        var frame = 0
        while (momentum.isActive && frame < 121) {
            val step = momentum.step(1.0 / 60.0)
            assertEquals(previous * 0.95f, momentum.velocity.x, 0.0001f)
            assertEquals(previous * 2f / 60f, step.dx, 0.0001f)
            assertEquals(0f, step.dy, EPSILON)
            if (step.finished) {
                assertTrue(momentum.velocity.x < 20f)
                assertEquals(3, step.phase)
            } else {
                assertEquals(if (frame == 0) 1 else 2, step.phase)
            }
            previous = momentum.velocity.x
            frame += 1
        }
        assertFalse(momentum.isActive)
        assertTrue(frame > 1)
        assertTrue(frame < 120)
    }

    @Test
    fun momentumThresholdDurationAndInterruption() {
        val momentum = ScrollMomentum()
        momentum.start(GPoint(50f, 0f))
        assertFalse(momentum.isActive)
        assertTrue(momentum.step(1.0 / 60.0).finished)
        momentum.start(GPoint(100f, 0f))
        assertFalse(momentum.isActive)
        momentum.start(GPoint(100_000f, 0f))
        val end = momentum.step(2.5)
        assertTrue(end.finished)
        assertEquals(3, end.phase)
        momentum.start(GPoint(0f, -600f))
        assertTrue(momentum.step(1.0 / 60.0).dy < 0f)
        assertTrue(momentum.stop())
        assertFalse(momentum.isActive)
        assertFalse(momentum.stop())
    }

    @Test
    fun momentumDecayIsIndependentOfFrameRate() {
        val slow = ScrollMomentum()
        val fast = ScrollMomentum()
        slow.start(GPoint(600f, 300f))
        fast.start(GPoint(600f, 300f))
        slow.step(1.0 / 30.0)
        fast.step(1.0 / 60.0)
        fast.step(1.0 / 60.0)
        assertPoint(slow.velocity, fast.velocity)
    }

    @Test
    fun momentumEndsAfter120FramesEvenAtHighSpeed() {
        val momentum = ScrollMomentum()
        momentum.start(GPoint(100_000f, 0f))
        repeat(119) { assertFalse(momentum.step(1.0 / 60.0).finished) }
        val end = momentum.step(1.0 / 60.0)
        assertTrue(end.finished)
        assertEquals(3, end.phase)
    }

    @Test
    fun pointerThrottleKeepsLatestAndFlushesOnce() {
        val throttle = PointerThrottle()
        val first = GPoint(0.1f, 0.1f)
        val last = GPoint(0.3f, 0.3f)
        assertEquals(first, throttle.offer(first, 0.0))
        assertNull(throttle.offer(GPoint(0.2f, 0.2f), 0.0005))
        assertNull(throttle.offer(last, 0.001))
        assertTrue(throttle.hasPending)
        assertEquals(last, throttle.flush(0.001))
        assertNull(throttle.flush(0.001))
        assertFalse(throttle.hasPending)
    }

    @Test
    fun pointerThrottleAllows120HzAndReplacesPendingSample() {
        val throttle = PointerThrottle()
        val point = GPoint(0.5f, 0.5f)
        assertEquals(GPoint.ZERO, throttle.offer(GPoint.ZERO, 0.0))
        assertNull(throttle.offer(GPoint.ZERO, 0.001))
        assertEquals(point, throttle.offer(point, 1.0 / 120.0))
        assertNull(throttle.flush(1.0 / 120.0))
        assertEquals(point, throttle.offer(point, 2.0 / 120.0))
    }

    @Test
    fun hostCursorArbitration() {
        val arbiter = HostCursorArbiter()
        assertTrue(arbiter.shouldAcceptHost(0.0))
        arbiter.recordLocalMove(1.0)
        assertFalse(arbiter.shouldAcceptHost(1.1))
        assertTrue(arbiter.shouldAcceptHost(1.25))
        arbiter.recordLocalMove(1.3)
        assertFalse(arbiter.shouldAcceptHost(1.4))
    }

    @Test
    fun viewportUnzoomedMapping() {
        val viewport = SessionViewport(contentRect = GRect(100f, 0f, 800f, 450f))
        val center = GPoint(0.5f, 0.5f)
        assertPoint(center, viewport.normalizedFromView(GPoint(500f, 225f)))
        assertPoint(GPoint(500f, 225f), viewport.viewPointFromNormalized(center))
        assertPoint(GPoint(0.1f, 0.1f), viewport.normalizedDeltaFromViewDelta(GPoint(80f, 45f)))
    }

    @Test
    fun viewportZoomClampsVisibleOffsetAndScalesDeltas() {
        val viewport =
            SessionViewport(
                contentRect = GRect(100f, 0f, 800f, 450f),
                zoom = 2f,
                offset = GPoint(0.25f, 0.25f),
            )
        val point = GPoint(500f, 225f)
        assertPoint(GPoint(0.5f, 0.5f), viewport.normalizedFromView(point))
        assertPoint(point, viewport.viewPointFromNormalized(viewport.normalizedFromView(point)))
        assertPoint(GPoint(0.05f, 0.05f), viewport.normalizedDeltaFromViewDelta(GPoint(80f, 45f)))
        viewport.offset = GPoint(1f, -1f)
        assertEquals(GPoint(0.5f, 0f), viewport.offset)
        viewport.zoom = 0f
        assertEquals(1f, viewport.zoom, EPSILON)
        assertEquals(GPoint.ZERO, viewport.offset)
    }

    @Test
    fun nonFiniteInputsFollowIosGuards() {
        assertEquals(1f, TrackpadAccel.gain(Float.NaN), EPSILON)
        assertEquals(3f, TrackpadAccel.gain(Float.POSITIVE_INFINITY), EPSILON)

        val cursor = CursorModel(GPoint(0.2f, 0.3f))
        cursor.moveTo(GPoint(Float.NaN, 1f))
        assertEquals(GPoint(0.2f, 0.3f), cursor.position)
        cursor.apply(GPoint(1f, 1f), SessionViewport(), Float.POSITIVE_INFINITY)
        assertEquals(GPoint(0.2f, 0.3f), cursor.position)

        val momentum = ScrollMomentum()
        momentum.start(GPoint(Float.POSITIVE_INFINITY, 0f))
        assertFalse(momentum.isActive)
    }

    @Test
    fun clockRollbackResetsClicksAndSuppressesHostPosition() {
        val clicks = ClickCounter()
        assertEquals(1, clicks.register(GPoint.ZERO, 5.0))
        assertEquals(1, clicks.register(GPoint.ZERO, 4.9))

        val arbiter = HostCursorArbiter()
        arbiter.recordLocalMove(5.0)
        assertFalse(arbiter.shouldAcceptHost(4.9))
        assertFalse(arbiter.shouldAcceptHost(Double.NaN))
    }

    private fun assertPoint(expected: GPoint, actual: GPoint) {
        assertEquals(expected.x, actual.x, EPSILON)
        assertEquals(expected.y, actual.y, EPSILON)
    }

    companion object {
        private const val EPSILON = 0.000001f
    }
}
