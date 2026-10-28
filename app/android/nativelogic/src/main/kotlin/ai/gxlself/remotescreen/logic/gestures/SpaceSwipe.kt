package ai.gxlself.remotescreen.logic.gestures

import kotlin.math.abs

/** Which way a three-finger swipe goes: fingers left bring in the next computer. */
enum class SpaceSwipeDirection(val value: String) {
  Previous("previous"),
  Next("next"),
}

/**
 * Pure decisions behind the three-finger "switch computer" swipe,
 * the same numbers as iOS NativeLogic SpaceSwipe. Distances in dp.
 */
object SpaceSwipe {
  const val SLOP_DP = 24f
  const val HORIZONTAL_BIAS = 1.2f
  const val COMMIT_FRACTION = 1f / 3f
  const val FLICK_DP_PER_S = 600f

  enum class Classification { Undecided, Swipe, Other }

  fun classify(dx: Float, dy: Float): Classification {
    val x = abs(dx)
    val y = abs(dy)
    if (x < SLOP_DP && y < SLOP_DP) return Classification.Undecided
    return if (x > y * HORIZONTAL_BIAS) Classification.Swipe else Classification.Other
  }

  /** What letting go does: switch to the neighbour, or null to stay. */
  fun decide(
    dx: Float,
    velocity: Float,
    width: Float,
    hasPrevious: Boolean,
    hasNext: Boolean,
  ): SpaceSwipeDirection? {
    if (width <= 0f || !dx.isFinite() || !velocity.isFinite() || abs(dx) < SLOP_DP) return null
    val direction = if (dx > 0) SpaceSwipeDirection.Previous else SpaceSwipeDirection.Next
    if (if (direction == SpaceSwipeDirection.Previous) !hasPrevious else !hasNext) return null
    val far = abs(dx) >= width * COMMIT_FRACTION
    val fast = abs(velocity) >= FLICK_DP_PER_S
    val flick = fast && (velocity > 0) == (dx > 0)
    val flungBack = fast && (velocity > 0) != (dx > 0)
    return if ((far && !flungBack) || flick) direction else null
  }

  /** The neighbours of [current] in [ids], without wrapping around. */
  fun neighbours(current: String, ids: List<String>): Pair<String?, String?> {
    val index = ids.indexOf(current)
    if (index < 0) return null to null
    return ids.getOrNull(index - 1) to ids.getOrNull(index + 1)
  }
}

/**
 * Follows the touches of one gesture and tells the view when three fingers
 * became a horizontal swipe (take the touch over, cancel what the remote got)
 * and, on release, whether to switch. Positions are the fingers' centroid in dp.
 */
class SpaceSwipeTracker {
  enum class Step {
    /** Not a swipe (yet): the other gestures handle it. */
    Pass,
    /** Just became a swipe: cancel the other gestures, consume from now on. */
    Began,
    /** Part of a swipe: consume. */
    Consumed,
  }

  private enum class State { Idle, Candidate, Swiping, Done, Ignored }

  private var state = State.Idle
  private var startX = 0f
  private var startY = 0f
  private var lastX = 0f
  private val samples = ArrayDeque<Pair<Long, Float>>()

  val swiping: Boolean
    get() = state == State.Swiping

  /** Every touch event: how many fingers are down and where they are. */
  fun track(pointers: Int, x: Float, y: Float, timeMs: Long): Step {
    when (state) {
      State.Idle -> {
        if (pointers == 3) {
          state = State.Candidate
          startX = x
          startY = y
        }
        return Step.Pass
      }
      State.Candidate -> {
        if (pointers != 3) {
          state = if (pointers > 3) State.Ignored else State.Idle
          return Step.Pass
        }
        return when (SpaceSwipe.classify(x - startX, y - startY)) {
          SpaceSwipe.Classification.Undecided -> Step.Pass
          SpaceSwipe.Classification.Other -> {
            state = State.Ignored
            Step.Pass
          }
          SpaceSwipe.Classification.Swipe -> {
            state = State.Swiping
            sample(x, timeMs)
            Step.Began
          }
        }
      }
      State.Swiping -> {
        if (pointers == 3) sample(x, timeMs)
        return Step.Consumed
      }
      State.Done -> return Step.Consumed
      State.Ignored -> return Step.Pass
    }
  }

  /** The first finger lifted during a swipe: switch or stay. */
  fun release(width: Float, hasPrevious: Boolean, hasNext: Boolean): SpaceSwipeDirection? {
    if (state != State.Swiping) return null
    state = State.Done
    return SpaceSwipe.decide(lastX - startX, velocity(), width, hasPrevious, hasNext)
  }

  /** The gesture is over (all fingers up or cancelled). */
  fun reset() {
    state = State.Idle
    samples.clear()
  }

  /** dp per second over the last 100 ms. */
  fun velocity(): Float {
    if (samples.size < 2) return 0f
    val (firstTime, firstX) = samples.first()
    val (lastTime, lastSampleX) = samples.last()
    val dt = lastTime - firstTime
    return if (dt <= 0) 0f else (lastSampleX - firstX) * 1000f / dt
  }

  private fun sample(x: Float, timeMs: Long) {
    lastX = x
    samples.addLast(timeMs to x)
    while (samples.size > 2 && timeMs - samples.first().first > 100) samples.removeFirst()
  }
}
