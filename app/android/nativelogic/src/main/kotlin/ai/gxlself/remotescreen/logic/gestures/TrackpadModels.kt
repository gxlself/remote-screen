package ai.gxlself.remotescreen.logic.gestures

import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.pow

class ClickCounter {
    private data class Click(val position: GPoint, val timeSec: Double, val count: Int)

    private var last: Click? = null

    fun register(position: GPoint, timeSec: Double): Int {
        val previous = last
        val count =
            if (
                previous != null &&
                    timeSec >= previous.timeSec &&
                    timeSec - previous.timeSec < CLICK_WINDOW_SEC &&
                    distance(position, previous.position) < CLICK_DISTANCE
            ) {
                min(previous.count + 1, MAX_CLICK_COUNT)
            } else {
                1
            }
        last = Click(position, timeSec, count)
        return count
    }

    private fun distance(a: GPoint, b: GPoint): Double =
        hypot((a.x - b.x).toDouble(), (a.y - b.y).toDouble())

    companion object {
        private const val CLICK_WINDOW_SEC = 0.4
        private const val CLICK_DISTANCE = 0.01
        private const val MAX_CLICK_COUNT = 3
    }
}

object TrackpadAccel {
    fun gain(speed: Float): Float {
        if (speed.isNaN()) return 1f
        return 1f + min(speed.coerceAtLeast(0f) / 900f, 2f)
    }
}

data class TrackpadPointerDelta(
    val delta: GPoint,
    val gain: Float,
)

/** Shared sensitivity and acceleration conversion for one-finger pointer motion. */
object TrackpadPointerMotion {
    fun calculate(delta: GPoint, speedDpPerSecond: Float, cursorSpeed: Float): TrackpadPointerDelta {
        val speed = cursorSpeed.takeIf { it.isFinite() }?.coerceIn(0.6f, 2.4f) ?: 1.4f
        val factor = speed / 1.4f
        return TrackpadPointerDelta(
            delta = GPoint(delta.x * factor, delta.y * factor),
            gain = TrackpadAccel.gain(speedDpPerSecond),
        )
    }
}

/** Tracks the left button held by a long-press drag and releases it once. */
class TrackpadDragState {
    var isDragging: Boolean = false
        private set

    fun start(): Boolean {
        if (isDragging) return false
        isDragging = true
        return true
    }

    fun release(): Boolean {
        if (!isDragging) return false
        isDragging = false
        return true
    }

    fun cancelForMultiFinger(): Boolean = release()

    fun reset() {
        isDragging = false
    }
}

class CursorModel(position: GPoint = GPoint(0.5f, 0.5f)) {
    var position: GPoint = GPoint(0.5f, 0.5f)
        private set

    init {
        moveTo(position)
    }

    fun moveTo(point: GPoint) {
        if (!point.x.isFinite() || !point.y.isFinite()) return
        position = GPoint(point.x.coerceIn(0f, 1f), point.y.coerceIn(0f, 1f))
    }

    fun apply(viewDelta: GPoint, viewport: SessionViewport, gain: Float) {
        if (!gain.isFinite() || gain < 0f) return
        val delta = viewport.normalizedDeltaFromViewDelta(viewDelta)
        moveTo(GPoint(position.x + delta.x * gain, position.y + delta.y * gain))
    }
}

class HostCursorArbiter {
    private var lastLocalMoveSec: Double? = null

    fun recordLocalMove(timeSec: Double) {
        if (timeSec.isFinite()) lastLocalMoveSec = timeSec
    }

    fun shouldAcceptHost(timeSec: Double): Boolean {
        if (!timeSec.isFinite()) return false
        val lastMove = lastLocalMoveSec ?: return true
        return timeSec - lastMove >= HOST_SUPPRESSION_SEC
    }

    companion object {
        private const val HOST_SUPPRESSION_SEC = 0.2
    }
}

class PointerThrottle {
    var lastEmissionTimeSec: Double? = null
        private set

    private var pending: GPoint? = null

    val hasPending: Boolean
        get() = pending != null

    fun offer(point: GPoint, nowSec: Double): GPoint? {
        if (!point.x.isFinite() || !point.y.isFinite() || !nowSec.isFinite()) return null
        pending = point
        val lastEmission = lastEmissionTimeSec
        if (lastEmission != null && nowSec - lastEmission < INTERVAL_SEC) return null
        return flush(nowSec)
    }

    fun flush(nowSec: Double): GPoint? {
        val point = pending ?: return null
        if (!nowSec.isFinite()) return null
        pending = null
        lastEmissionTimeSec = nowSec
        return point
    }

    companion object {
        const val INTERVAL_SEC = 1.0 / 120.0
    }
}

data class ScrollStep(
    val dx: Float,
    val dy: Float,
    val phase: Int,
    val finished: Boolean,
)

class ScrollMomentum {
    var velocity: GPoint = GPoint.ZERO
        private set
    var isActive: Boolean = false
        private set

    private var elapsedSec = 0.0
    private var firstFrame = false

    fun start(velocity: GPoint) {
        this.velocity = velocity
        elapsedSec = 0.0
        isActive =
            velocity.x.isFinite() &&
            velocity.y.isFinite() &&
            hypot(velocity.x.toDouble(), velocity.y.toDouble()) > START_SPEED
        firstFrame = isActive
    }

    fun step(dtSec: Double): ScrollStep {
        if (!isActive) return ScrollStep(0f, 0f, PHASE_END, true)
        if (!dtSec.isFinite() || dtSec <= 0.0) return ScrollStep(0f, 0f, PHASE_NONE, false)

        val deltaTime = min(dtSec, (MAX_DURATION_SEC - elapsedSec).coerceAtLeast(0.0))
        val dx = velocity.x * deltaTime.toFloat() * DISTANCE_SCALE
        val dy = velocity.y * deltaTime.toFloat() * DISTANCE_SCALE
        val decay = DECAY_PER_FRAME.pow(deltaTime * FRAMES_PER_SEC).toFloat()
        velocity = GPoint(velocity.x * decay, velocity.y * decay)
        elapsedSec += deltaTime

        if (
            hypot(velocity.x.toDouble(), velocity.y.toDouble()) < STOP_SPEED ||
                elapsedSec >= MAX_DURATION_SEC - END_EPSILON_SEC
        ) {
            isActive = false
            return ScrollStep(dx, dy, PHASE_END, true)
        }

        val phase = if (firstFrame) PHASE_BEGIN else PHASE_CONTINUE
        firstFrame = false
        return ScrollStep(dx, dy, phase, false)
    }

    fun stop(): Boolean {
        val wasActive = isActive
        isActive = false
        velocity = GPoint.ZERO
        return wasActive
    }

    companion object {
        private const val START_SPEED = 100.0
        private const val STOP_SPEED = 20.0
        private const val DISTANCE_SCALE = 2f
        private const val DECAY_PER_FRAME = 0.95
        private const val FRAMES_PER_SEC = 60.0
        private const val MAX_DURATION_SEC = 2.0
        private const val END_EPSILON_SEC = 1e-9
        private const val PHASE_NONE = 0
        private const val PHASE_BEGIN = 1
        private const val PHASE_CONTINUE = 2
        private const val PHASE_END = 3
    }
}
