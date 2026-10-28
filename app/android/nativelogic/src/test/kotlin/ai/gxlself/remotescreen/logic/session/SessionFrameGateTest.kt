package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionFrameGateTest {
    @Test
    fun normalFramesLeaveInFifoOrder() {
        val gate = SessionFrameGate<String>(maxBacklog = 3)
        assertEquals(emptyList<GateAction>(), gate.push(meta(1, keyframe = true), "one", 0))
        gate.push(meta(2), "two", 1)
        gate.push(meta(3), "three", 2)
        assertEquals(1, gate.next()?.first?.frameId)
        assertEquals("two", gate.next()?.second)
        assertEquals(3, gate.next()?.first?.frameId)
        assertNull(gate.next())
    }

    @Test
    fun fourthDeltaFrameDropsEntireBacklogAndRequestsKeyframe() {
        val gate = SessionFrameGate<String>(maxBacklog = 3)
        for (id in 1..3) gate.push(meta(id), id.toString(), id.toLong())
        assertEquals(
            listOf(
                GateAction.Drop(1),
                GateAction.Drop(2),
                GateAction.Drop(3),
                GateAction.Drop(4),
                GateAction.RequestKeyframe(KEYFRAME_BACKLOG),
            ),
            gate.push(meta(4), "4", 4),
        )
        assertEquals(0, gate.pending)
        assertTrue(gate.waitingForKeyframe)
    }

    @Test
    fun overflowingParameterSetKeyframeDropsOnlyOlderFrames() {
        val gate = SessionFrameGate<String>(maxBacklog = 2)
        gate.push(meta(1), "one", 0)
        gate.push(meta(2), "two", 1)
        assertEquals(
            listOf(GateAction.Drop(1), GateAction.Drop(2)),
            gate.push(meta(3, keyframe = true, parameterSets = true), "three", 2),
        )
        assertEquals(3, gate.next()?.first?.frameId)
        assertFalse(gate.waitingForKeyframe)
        assertEquals(0, gate.keyframeRequests)
    }

    @Test
    fun waitingDropsDeltaFrame() {
        val gate = waitingGate()
        assertEquals(listOf(GateAction.Drop(2)), gate.push(meta(2), Unit, 10))
    }

    @Test
    fun waitingDropsKeyframeWithoutParameterSets() {
        val gate = waitingGate()
        assertEquals(listOf(GateAction.Drop(2)), gate.push(meta(2, keyframe = true), Unit, 10))
        assertTrue(gate.waitingForKeyframe)
    }

    @Test
    fun parameterSetKeyframeLeavesWaitingAndQueues() {
        val gate = waitingGate()
        assertEquals(emptyList<GateAction>(), gate.push(meta(2, true, true), Unit, 10))
        assertFalse(gate.waitingForKeyframe)
        assertEquals(2, gate.next()?.first?.frameId)
    }

    @Test
    fun nonKeyframeWhileDecoderNotReadyDropsPendingAndCurrent() {
        val gate = SessionFrameGate<Unit>()
        gate.push(meta(1, true, true), Unit, 0)
        gate.decoderReady(false)
        assertEquals(
            listOf(GateAction.Drop(1), GateAction.Drop(2), GateAction.RequestKeyframe(4)),
            gate.push(meta(2), Unit, 1),
        )
    }

    @Test
    fun decodeFailureDropsQueueAndRequestsDecoderReset() {
        val gate = SessionFrameGate<Unit>()
        gate.push(meta(1), Unit, 0)
        assertEquals(
            listOf(GateAction.Drop(1), GateAction.RequestKeyframe(KEYFRAME_DECODER_RESET)),
            gate.decodeFailed(10),
        )
    }

    @Test
    fun tickRerequestsAtFiveHundredMilliseconds() {
        val gate = SessionFrameGate<Unit>()
        assertEquals(listOf(GateAction.RequestKeyframe(2)), gate.decodeFailed(0))
        assertEquals(emptyList<GateAction>(), gate.tick(499))
        assertEquals(listOf(GateAction.RequestKeyframe(2)), gate.tick(500))
    }

    @Test
    fun throttledFailureIsSentWhenMinimumIntervalExpires() {
        val gate = SessionFrameGate<Unit>()
        assertEquals(listOf(GateAction.RequestKeyframe(2)), gate.decodeFailed(0))
        assertEquals(emptyList<GateAction>(), gate.decodeFailed(200))
        assertEquals(emptyList<GateAction>(), gate.tick(299))
        assertEquals(listOf(GateAction.RequestKeyframe(2)), gate.tick(300))
    }

    @Test
    fun resetWaitsBeforeRequestingBacklogKeyframe() {
        val gate = SessionFrameGate<Unit>()
        gate.reset(waitForKeyframe = true, nowMs = 0)
        assertEquals(emptyList<GateAction>(), gate.tick(499))
        assertEquals(listOf(GateAction.RequestKeyframe(4)), gate.tick(500))
    }

    @Test
    fun recoveryKeyframeCancelsResetRequest() {
        val gate = SessionFrameGate<Unit>()
        gate.reset(waitForKeyframe = true, nowMs = 0)
        gate.push(meta(1, true, true), Unit, 100)
        assertEquals(emptyList<GateAction>(), gate.tick(500))
    }

    @Test
    fun resetWithoutWaitingClearsQueue() {
        val gate = SessionFrameGate<Unit>()
        gate.push(meta(1), Unit, 0)
        gate.reset(waitForKeyframe = false, nowMs = 1)
        assertEquals(0, gate.pending)
        assertFalse(gate.waitingForKeyframe)
        assertEquals(1, gate.dropped)
    }

    @Test
    fun countersAccumulateAndCanBeReset() {
        val gate = SessionFrameGate<Unit>(maxBacklog = 0)
        gate.push(meta(1), Unit, 0)
        gate.push(meta(2), Unit, 1)
        assertEquals(2, gate.dropped)
        assertEquals(1, gate.keyframeRequests)
        gate.resetCounters()
        assertEquals(0, gate.dropped)
        assertEquals(0, gate.keyframeRequests)
    }

    private fun waitingGate(): SessionFrameGate<Unit> =
        SessionFrameGate<Unit>().also { it.decodeFailed(0) }

    private fun meta(
        id: Int,
        keyframe: Boolean = false,
        parameterSets: Boolean = false,
    ) = FrameMeta(id, epoch = 1, keyframe = keyframe, hasParameterSets = parameterSets)
}
