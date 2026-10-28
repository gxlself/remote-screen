package ai.gxlself.remotescreen.rsnative.session.gestures

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Choreographer
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.ViewConfiguration
import ai.gxlself.remotescreen.logic.gestures.ClickCounter
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.MultiFingerTaps
import ai.gxlself.remotescreen.logic.gestures.ScrollMomentum
import ai.gxlself.remotescreen.logic.gestures.TrackpadDragState
import ai.gxlself.remotescreen.logic.gestures.TrackpadPointerMotion
import ai.gxlself.remotescreen.logic.gestures.TapResult
import java.util.LinkedHashMap
import kotlin.math.hypot

/** Android implementation of the 07 trackpad gesture column. */
class TrackpadGestures(
  private val view: android.view.View,
  private val input: SessionInputSender,
  private val cursor: SessionCursorController,
  private val zoomHandlerProvider: () -> ZoomHandler?,
  private val emitGesture: (String) -> Unit,
  private val cursorSpeedProvider: () -> Float,
  private val naturalScrollProvider: () -> Boolean,
  private val hapticsProvider: () -> Boolean,
  private val sessionReadyProvider: () -> Boolean,
) {
  /** The platform timeout used because GestureDetector has no public setter. */
  val actualLongPressMs: Long = ViewConfiguration.getLongPressTimeout().toLong()

  private val density = view.resources.displayMetrics.density.takeIf { it.isFinite() && it > 0f } ?: 1f
  private val handler = Handler(Looper.getMainLooper())
  private var taps = MultiFingerTaps()
  private var clicks = ClickCounter()
  private val momentum = ScrollMomentum()
  private val pointers = LinkedHashMap<Int, GPoint>()
  private val downPointers = LinkedHashMap<Int, GPoint>()
  private var velocityTracker: android.view.VelocityTracker? = null
  private val gestureDetector: GestureDetector
  private val scaleDetector: ScaleGestureDetector

  private var moving = false
  private val drag = TrackpadDragState()
  private var scrollActive = false
  private var pinching = false
  private var pinchScale = 1f
  private var singleCancelledByMulti = false
  private var hadMultiFinger = false
  private var twoFingerTapCandidate = false
  private var lastCentroid = GPoint.ZERO
  private var twoFingerAnchor = GPoint.ZERO
  private var lastCentroidTimeMs = 0L
  private var lastScrollVelocity = GPoint.ZERO
  private var tapTickDeadlineMs = 0L
  private var tapTick: Runnable? = null
  private var momentumFramePosted = false
  private var lastMomentumNanos = 0L
  private var closed = false

  private val dragging: Boolean
    get() = drag.isDragging

  private val momentumFrame = Choreographer.FrameCallback { frameNanos ->
    momentumFramePosted = false
    if (!momentum.isActive) return@FrameCallback
    val dt = if (lastMomentumNanos == 0L) 1.0 / 60.0
    else ((frameNanos - lastMomentumNanos).coerceAtLeast(1L) / 1_000_000_000.0)
    lastMomentumNanos = frameNanos
    val step = momentum.step(dt)
    if (step.phase != PHASE_NONE) {
      sendScroll(step.dx, step.dy, phase = PHASE_NONE, momentumPhase = step.phase)
    }
    if (step.finished) {
      lastMomentumNanos = 0L
    } else {
      postMomentumFrame()
    }
  }

  init {
    gestureDetector = GestureDetector(view.context, object : GestureDetector.SimpleOnGestureListener() {
      override fun onDown(event: MotionEvent): Boolean = true

      override fun onSingleTapUp(event: MotionEvent): Boolean {
        if (!canInteract() || taps.isMultiFinger || singleCancelledByMulti) return true
        val count = clicks.register(cursor.position, nowSeconds())
        input.click(BUTTON_LEFT, cursor.position, count)
        emitGesture(if (count >= 2) "doubleClick" else "click")
        return true
      }

      override fun onScroll(
        first: MotionEvent?, current: MotionEvent,
        distanceX: Float, distanceY: Float,
      ): Boolean {
        if (!canInteract() || taps.isMultiFinger || pointers.size != 1 || pinching) return true
        val delta = GPoint(-distanceX / density, -distanceY / density)
        if (!delta.x.isFinite() || !delta.y.isFinite()) return true
        if (dragging) return true
        if (!moving) moving = true
        movePointer(delta)
        input.pointer(cursor.position)
        return true
      }

      override fun onLongPress(event: MotionEvent) {
        if (!canInteract() || taps.isMultiFinger || singleCancelledByMulti || dragging) return
        if (!drag.start()) return
        moving = false
        performLongPressHaptic()
        input.button(BUTTON_LEFT, true, cursor.position)
      }
    }).apply {
      setIsLongpressEnabled(true)
    }

    scaleDetector = ScaleGestureDetector(view.context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
      override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
        pinchScale = 1f
        return true
      }

      override fun onScale(detector: ScaleGestureDetector): Boolean {
        val factor = detector.scaleFactor
        if (!factor.isFinite() || factor <= 0f) return true
        pinchScale *= factor
        val scale = pinchScale
        if (!scale.isFinite()) return true
        if (!pinching && kotlin.math.abs(scale - 1f) > PINCH_THRESHOLD) {
          pinching = true
          if (scrollActive) finishScroll(startMomentumAfter = false, report = false)
          zoomHandlerProvider()?.onPinch(
            scale,
            detector.focusX / density,
            detector.focusY / density,
            PHASE_BEGIN,
          )
        } else if (pinching) {
          zoomHandlerProvider()?.onPinch(
            scale,
            detector.focusX / density,
            detector.focusY / density,
            PHASE_CHANGED,
          )
        }
        return true
      }

      override fun onScaleEnd(detector: ScaleGestureDetector) {
        if (pinching) {
          zoomHandlerProvider()?.onPinch(
            pinchScale,
            detector.focusX / density,
            detector.focusY / density,
            PHASE_END,
          )
        }
        pinching = false
        pinchScale = 1f
      }
    })
  }

  fun onTouch(event: MotionEvent): Boolean {
    if (closed) return true
    if (!canInteract()) {
      if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
        recycleVelocityTracker()
      }
      return true
    }
    if (event.actionMasked == MotionEvent.ACTION_DOWN && velocityTracker == null) {
      velocityTracker = android.view.VelocityTracker.obtain()
    }
    velocityTracker?.addMovement(event)
    when (event.actionMasked) {
      MotionEvent.ACTION_DOWN,
      MotionEvent.ACTION_POINTER_DOWN,
      MotionEvent.ACTION_MOVE -> updateRawBeforeDetectors(event)
    }
    scaleDetector.onTouchEvent(event)
    gestureDetector.onTouchEvent(event)
    when (event.actionMasked) {
      MotionEvent.ACTION_POINTER_UP,
      MotionEvent.ACTION_UP,
      MotionEvent.ACTION_CANCEL -> updateRawAfterDetectors(event)
    }
    if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
      recycleVelocityTracker()
    }
    return true
  }

  fun cancel() {
    cancelDetectors()
    if (dragging) {
      input.flushPointer()
      input.button(BUTTON_LEFT, false, cursor.position)
      emitGesture("drag")
    } else if (moving) {
      input.flushPointer()
      emitGesture("move")
    }
    if (scrollActive) finishScroll(startMomentumAfter = false, report = true)
    stopMomentum()
    input.releaseAll()
    cancelTapTick()
    taps = MultiFingerTaps()
    clicks = ClickCounter()
    resetRawState()
    recycleVelocityTracker()
  }

  private fun cancelDetectors() {
    val now = SystemClock.uptimeMillis()
    val event = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0f, 0f, 0)
    try {
      scaleDetector.onTouchEvent(event)
      gestureDetector.onTouchEvent(event)
    } finally {
      event.recycle()
    }
  }

  fun close() {
    if (closed) return
    cancel()
    closed = true
    recycleVelocityTracker()
  }

  private fun updateRawBeforeDetectors(event: MotionEvent) {
    when (event.actionMasked) {
      MotionEvent.ACTION_DOWN -> {
        stopMomentum()
        pointers.clear()
        downPointers.clear()
        val singlePointerId = event.getPointerId(0)
        moving = false
        drag.reset()
        scrollActive = false
        pinching = false
        singleCancelledByMulti = false
        hadMultiFinger = false
        twoFingerTapCandidate = false
        lastScrollVelocity = GPoint.ZERO
        processTapResults(taps.down(singlePointerId, xDp(event.getX(0)), yDp(event.getY(0)), event.eventTime))
        pointers[singlePointerId] = eventPoint(event, 0)
      }
      MotionEvent.ACTION_POINTER_DOWN -> {
        val index = event.actionIndex
        val id = event.getPointerId(index)
        val point = eventPoint(event, index)
        processTapResults(taps.down(id, point.x, point.y, event.eventTime))
        pointers[id] = point
        downPointers[id] = point
        if (pointers.size >= 2) {
          hadMultiFinger = true
          if (!singleCancelledByMulti) {
            singleCancelledByMulti = true
            finishSingleGesture(forMultiFinger = true)
          }
          if (pointers.size == 2 && !pinching) {
            twoFingerTapCandidate = true
            scrollActive = false
            twoFingerAnchor = centroid()
            lastCentroid = centroid()
            lastCentroidTimeMs = event.eventTime
            lastScrollVelocity = GPoint.ZERO
          } else if (pointers.size > 2 && scrollActive) {
            twoFingerTapCandidate = false
            finishScroll(startMomentumAfter = false, report = true)
          } else if (pointers.size > 2) {
            twoFingerTapCandidate = false
          }
        }
      }
      MotionEvent.ACTION_MOVE -> {
        val singlePointerId = event.getPointerId(0).takeIf { event.pointerCount == 1 && pointers.size == 1 }
        val previousSingle = singlePointerId?.let(pointers::get)
        for (index in 0 until event.pointerCount) {
          val id = event.getPointerId(index)
          val point = eventPoint(event, index)
          pointers[id] = point
          val down = downPointers[id]
          if (down != null && hypot((point.x - down.x).toDouble(), (point.y - down.y).toDouble()) > TWO_FINGER_SCROLL_SLOP_DP) {
            twoFingerTapCandidate = false
          }
          processTapResults(taps.move(id, point.x, point.y, event.eventTime))
        }
        if (dragging && event.pointerCount == 1 && pointers.size == 1 && previousSingle != null) {
          val current = singlePointerId?.let(pointers::get)
          if (current != null) {
            movePointer(GPoint(current.x - previousSingle.x, current.y - previousSingle.y))
            input.pointer(cursor.position)
          }
        }
        if (pointers.size >= 2 && !pinching) {
          val current = centroid()
          val dtMs = (event.eventTime - lastCentroidTimeMs).coerceAtLeast(1L)
          val delta = GPoint(current.x - lastCentroid.x, current.y - lastCentroid.y)
          if (!scrollActive && hypot(
              (current.x - twoFingerAnchor.x).toDouble(),
              (current.y - twoFingerAnchor.y).toDouble(),
            ) > TWO_FINGER_SCROLL_SLOP_DP
          ) {
            scrollActive = true
            sendScroll(0f, 0f, phase = PHASE_BEGIN)
          }
          if (scrollActive) {
            lastScrollVelocity = GPoint(delta.x * 1_000f / dtMs, delta.y * 1_000f / dtMs)
            sendScroll(delta.x * 2f, delta.y * 2f, phase = PHASE_CHANGED)
          }
          lastCentroid = current
          lastCentroidTimeMs = event.eventTime
        }
      }
    }
  }

  private fun updateRawAfterDetectors(event: MotionEvent) {
    when (event.actionMasked) {
      MotionEvent.ACTION_POINTER_UP -> {
        val index = event.actionIndex
        val id = event.getPointerId(index)
        processTapResults(taps.up(id, event.eventTime))
        pointers.remove(id)
        downPointers.remove(id)
        if (scrollActive && pointers.size < 2) finishScroll(startMomentumAfter = true, report = true)
      }
      MotionEvent.ACTION_UP -> {
        val id = event.getPointerId(0)
        processTapResults(taps.up(id, event.eventTime))
        pointers.clear()
        if (scrollActive) finishScroll(startMomentumAfter = true, report = true)
        finishSingleGesture()
        if (hadMultiFinger && twoFingerTapCandidate) scheduleTapTick()
      }
      MotionEvent.ACTION_CANCEL -> {
        taps.cancel()
        pointers.clear()
        cancelTapTick()
        finishSingleGesture()
        if (scrollActive) finishScroll(startMomentumAfter = false, report = true)
        stopMomentum()
        input.releaseAll()
        taps = MultiFingerTaps()
        clicks = ClickCounter()
        resetRawState()
      }
    }
  }

  private fun finishSingleGesture(forMultiFinger: Boolean = false) {
    if (dragging) {
      if (forMultiFinger) drag.cancelForMultiFinger() else drag.release()
      input.flushPointer()
      input.button(BUTTON_LEFT, false, cursor.position)
      emitGesture("drag")
    } else if (moving) {
      moving = false
      input.flushPointer()
      emitGesture("move")
    }
  }

  private fun finishScroll(startMomentumAfter: Boolean, report: Boolean) {
    if (!scrollActive) return
    scrollActive = false
    sendScroll(0f, 0f, phase = PHASE_END)
    if (report) emitGesture("scroll")
    if (startMomentumAfter && !pinching) startMomentum()
  }

  private fun startMomentum() {
    stopMomentum()
    momentum.start(lastScrollVelocity)
    if (!momentum.isActive) return
    lastMomentumNanos = 0L
    postMomentumFrame()
  }

  private fun postMomentumFrame() {
    if (momentumFramePosted || !momentum.isActive) return
    momentumFramePosted = true
    Choreographer.getInstance().postFrameCallback(momentumFrame)
  }

  private fun stopMomentum() {
    val wasActive = momentum.stop()
    if (momentumFramePosted) {
      Choreographer.getInstance().removeFrameCallback(momentumFrame)
      momentumFramePosted = false
    }
    lastMomentumNanos = 0L
    if (wasActive) input.scroll(0f, 0f, phase = PHASE_NONE, momentum = PHASE_END)
  }

  private fun scheduleTapTick() {
    cancelTapTick()
    tapTickDeadlineMs = SystemClock.uptimeMillis() + TAP_TICK_WINDOW_MS + TAP_TICK_INTERVAL_MS
    val runnable = object : Runnable {
      override fun run() {
        tapTick = null
        val now = SystemClock.uptimeMillis()
        processTapResults(taps.tick(now))
        if (now < tapTickDeadlineMs) {
          tapTick = this
          handler.postDelayed(this, TAP_TICK_INTERVAL_MS)
        }
      }
    }
    tapTick = runnable
    handler.postDelayed(runnable, TAP_TICK_INTERVAL_MS)
  }

  private fun cancelTapTick() {
    tapTick?.let(handler::removeCallbacks)
    tapTick = null
  }

  private fun processTapResults(results: List<TapResult>) {
    if (results.isEmpty()) return
    cancelTapTick()
    results.forEach { result ->
      when (result) {
        is TapResult.TwoFingerTap -> {
          input.click(BUTTON_RIGHT, cursor.position)
          emitGesture("rightClick")
        }
        TapResult.TwoFingerDoubleTap -> emitGesture("twoFingerDoubleTap")
        TapResult.ThreeFingerTap -> emitGesture("threeFingerTap")
      }
    }
  }

  private fun sendScroll(dx: Float, dy: Float, phase: Int = PHASE_NONE, momentumPhase: Int = PHASE_NONE) {
    val natural = naturalScrollProvider()
    val sign = if (natural) 1f else -1f
    input.scroll(dx * sign, dy * sign, phase, momentumPhase)
  }

  private fun currentSpeedDpPerSecond(): Float {
    val tracker = velocityTracker ?: return 0f
    tracker.computeCurrentVelocity(1000)
    val vx = tracker.xVelocity / density
    val vy = tracker.yVelocity / density
    return hypot(vx.toDouble(), vy.toDouble()).toFloat()
  }

  private fun movePointer(delta: GPoint) {
    if (!delta.x.isFinite() || !delta.y.isFinite()) return
    val motion = TrackpadPointerMotion.calculate(
      delta = delta,
      speedDpPerSecond = currentSpeedDpPerSecond(),
      cursorSpeed = cursorSpeedProvider(),
    )
    cursor.moveBy(motion.delta, motion.gain)
  }

  private fun recycleVelocityTracker() {
    velocityTracker?.recycle()
    velocityTracker = null
  }

  private fun eventPoint(event: MotionEvent, index: Int): GPoint =
    GPoint(xDp(event.getX(index)), yDp(event.getY(index)))

  private fun centroid(): GPoint {
    if (pointers.isEmpty()) return GPoint.ZERO
    val first = pointers.values.take(2)
    return GPoint(first.sumOf { it.x.toDouble() }.toFloat() / first.size,
      first.sumOf { it.y.toDouble() }.toFloat() / first.size)
  }

  private fun xDp(px: Float): Float = px / density

  private fun yDp(px: Float): Float = px / density

  private fun canInteract(): Boolean = sessionReadyProvider() && view.isShown

  private fun performLongPressHaptic() {
    if (hapticsProvider()) view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
  }

  private fun resetRawState() {
    pointers.clear()
    downPointers.clear()
    moving = false
    drag.reset()
    scrollActive = false
    pinching = false
    pinchScale = 1f
    singleCancelledByMulti = false
    hadMultiFinger = false
    twoFingerTapCandidate = false
    lastScrollVelocity = GPoint.ZERO
  }

  private fun nowSeconds(): Double = System.nanoTime() / 1_000_000_000.0

  companion object {
    private const val BUTTON_LEFT = 0
    private const val BUTTON_RIGHT = 1
    private const val PINCH_THRESHOLD = 0.05f
    private const val TWO_FINGER_SCROLL_SLOP_DP = 10f
    private const val TAP_TICK_INTERVAL_MS = 50L
    private const val TAP_TICK_WINDOW_MS = 300L
    private const val PHASE_NONE = 0
    private const val PHASE_BEGIN = 1
    private const val PHASE_CHANGED = 2
    private const val PHASE_END = 3
  }
}
