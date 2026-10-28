package ai.gxlself.remotescreen.logic.gestures

import kotlin.math.hypot

sealed interface TapResult {
    data class TwoFingerTap(val cx: Float, val cy: Float) : TapResult

    data object TwoFingerDoubleTap : TapResult

    data object ThreeFingerTap : TapResult
}

class MultiFingerTaps(
    val maxTapMs: Long = 300,
    val slopDp: Float = 10f,
    val doubleTapWindowMs: Long = 300,
) {
    private data class Finger(val down: GPoint)

    private data class Sequence(
        val startMs: Long,
        val fingers: MutableMap<Int, Finger>,
        val firstDowns: MutableList<GPoint>,
        var maxFingerCount: Int,
        var invalid: Boolean,
    )

    private data class PendingTap(val center: GPoint, val upMs: Long)

    private var sequence: Sequence? = null
    private var pendingTap: PendingTap? = null
    private var latestSequenceWasMultiFinger = false

    val isMultiFinger: Boolean
        get() = latestSequenceWasMultiFinger

    fun down(pointerId: Int, x: Float, y: Float, tMs: Long): List<TapResult> {
        val results = mutableListOf<TapResult>()

        val point = GPoint(x, y)
        var current = sequence
        if (current == null) {
            latestSequenceWasMultiFinger = false
            current =
                Sequence(
                    startMs = tMs,
                    fingers = linkedMapOf(),
                    firstDowns = mutableListOf(),
                    maxFingerCount = 0,
                    invalid = false,
                )
            sequence = current
        }

        if (current.fingers.containsKey(pointerId)) {
            current.invalid = true
        } else {
            current.fingers[pointerId] = Finger(point)
            current.firstDowns += point
            current.maxFingerCount = maxOf(current.maxFingerCount, current.fingers.size)
            latestSequenceWasMultiFinger = current.maxFingerCount >= 2
        }
        if (!x.isFinite() || !y.isFinite()) current.invalid = true
        if (!isValidElapsed(current.startMs, tMs, maxTapMs)) current.invalid = true

        if (current.invalid || current.maxFingerCount > 2) emitPending(results)
        expirePending(tMs, results, deadlineIsInclusive = false)
        return results
    }

    fun move(pointerId: Int, x: Float, y: Float, tMs: Long): List<TapResult> {
        val results = mutableListOf<TapResult>()
        val current = sequence
        val finger = current?.fingers?.get(pointerId)
        if (current != null && finger != null) {
            if (
                !x.isFinite() ||
                    !y.isFinite() ||
                    hypot((x - finger.down.x).toDouble(), (y - finger.down.y).toDouble()) > slopDp ||
                    !isValidElapsed(current.startMs, tMs, maxTapMs)
            ) {
                current.invalid = true
                emitPending(results)
            }
        }
        expirePending(tMs, results)
        return results
    }

    fun up(pointerId: Int, tMs: Long): List<TapResult> {
        val results = mutableListOf<TapResult>()
        val current = sequence
        if (current == null || current.fingers.remove(pointerId) == null) {
            expirePending(tMs, results)
            return results
        }

        if (!isValidElapsed(current.startMs, tMs, maxTapMs)) current.invalid = true
        if (current.fingers.isNotEmpty()) {
            expirePending(tMs, results)
            return results
        }

        sequence = null
        if (current.invalid) {
            emitPending(results)
            return results
        }

        when (current.maxFingerCount) {
            2 -> finishTwoFingerTap(current, tMs, results)
            3 -> {
                emitPending(results)
                results += TapResult.ThreeFingerTap
            }
            else -> emitPending(results)
        }
        return results
    }

    fun cancel() {
        sequence = null
        latestSequenceWasMultiFinger = false
    }

    fun tick(tMs: Long): List<TapResult> {
        val results = mutableListOf<TapResult>()
        val current = sequence
        if (current != null && !isValidElapsed(current.startMs, tMs, maxTapMs)) {
            current.invalid = true
            emitPending(results)
        }
        expirePending(tMs, results)
        return results
    }

    private fun finishTwoFingerTap(
        current: Sequence,
        upMs: Long,
        results: MutableList<TapResult>,
    ) {
        val previous = pendingTap
        if (previous != null && isValidElapsed(previous.upMs, current.startMs, doubleTapWindowMs)) {
            pendingTap = null
            results += TapResult.TwoFingerDoubleTap
            return
        }

        emitPending(results)
        val first = current.firstDowns[0]
        val second = current.firstDowns[1]
        pendingTap =
            PendingTap(
                center = GPoint((first.x + second.x) / 2f, (first.y + second.y) / 2f),
                upMs = upMs,
            )
    }

    private fun expirePending(
        tMs: Long,
        results: MutableList<TapResult>,
        deadlineIsInclusive: Boolean = true,
    ) {
        val pending = pendingTap ?: return
        if (!hasReachedDeadline(pending.upMs, tMs, doubleTapWindowMs, deadlineIsInclusive)) return
        val current = sequence
        val potentialDoubleTap =
            current != null &&
                !current.invalid &&
                current.maxFingerCount <= 2 &&
                isValidElapsed(pending.upMs, current.startMs, doubleTapWindowMs)
        if (potentialDoubleTap) return
        emitPending(results)
    }

    private fun emitPending(results: MutableList<TapResult>) {
        val pending = pendingTap ?: return
        pendingTap = null
        results += TapResult.TwoFingerTap(pending.center.x, pending.center.y)
    }

    private fun isValidElapsed(startMs: Long, endMs: Long, maximumMs: Long): Boolean {
        if (maximumMs < 0 || endMs < startMs) return false
        val elapsed = endMs - startMs
        return elapsed >= 0 && elapsed <= maximumMs
    }

    private fun hasReachedDeadline(
        startMs: Long,
        endMs: Long,
        windowMs: Long,
        inclusive: Boolean,
    ): Boolean {
        if (windowMs < 0 || endMs < startMs) return true
        val elapsed = endMs - startMs
        if (elapsed < 0) return true
        return if (inclusive) elapsed >= windowMs else elapsed > windowMs
    }
}
