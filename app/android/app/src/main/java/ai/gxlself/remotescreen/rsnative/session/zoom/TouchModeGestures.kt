package ai.gxlself.remotescreen.rsnative.session.zoom

import android.graphics.Canvas
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Choreographer
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.VelocityTracker
import android.view.ViewConfiguration
import ai.gxlself.remotescreen.logic.gestures.ClickCounter
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.MultiFingerTaps
import ai.gxlself.remotescreen.logic.gestures.ScrollMomentum
import ai.gxlself.remotescreen.logic.gestures.TapResult
import ai.gxlself.remotescreen.logic.zoom.TouchTapMapper
import ai.gxlself.remotescreen.logic.zoom.Viewport
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import ai.gxlself.remotescreen.rsnative.session.gestures.SessionGestureController
import ai.gxlself.remotescreen.rsnative.session.gestures.SessionInputSender
import java.util.LinkedHashMap
import kotlin.math.hypot

/** Direct-touch gesture column: one finger controls the remote pointer directly. */
class TouchModeGestures(
  private val view: RemoteSessionView,
  private val controller: SessionGestureController,
  zoomController: ZoomController? = null,
) : RemoteSessionView.SessionGestureLayer {
  private val zoom: ZoomController =
    zoomController ?: (controller.zoomHandler as? ZoomController ?: ZoomController(view))
  private val density = view.resources.displayMetrics.density
    .takeIf { it.isFinite() && it > 0f } ?: 1f
  private val input = SessionInputSender { view.sessionLong }
  private val handler = Handler(Looper.getMainLooper())
  private val velocityTracker = VelocityTracker.obtain()
  private val momentum = ScrollMomentum()
  private val pointers = LinkedHashMap<Int, GPoint>()
  private val downPointers = LinkedHashMap<Int, GPoint>()
  private var taps = MultiFingerTaps()
  private var clicks = ClickCounter()
  private var contentRectDp = GRect.ZERO
  private var cursor = GPoint(0.5f, 0.5f)
  private var finger = GPoint.ZERO
  private var leftDown = false
  private var moved = false
  private var longPressed = false
  private var longCancelled = false
  private var multiFinger = false
  private var twoFingerTapCandidate = false
  private var pinching = false
  private var pinchScale = 1f
  private var lastCentroid = GPoint.ZERO
  private var twoFingerAnchor = GPoint.ZERO
  private var lastCentroidTimeMs = 0L
  private var scrollActive = false
  private var lastScrollVelocity = GPoint.ZERO
  private var longPressRunnable: Runnable? = null
  private var momentumFramePosted = false
  private var lastMomentumNanos = 0L
  private val magnifier = SessionMagnifier(view) { controller.viewport }
  private val scaleDetector = ScaleGestureDetector(
    view.context,
    object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
      override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
        pinchScale = 1f
        return true
      }

      override fun onScale(detector: ScaleGestureDetector): Boolean {
        val factor = detector.scaleFactor
        if (!factor.isFinite() || factor <= 0f) return true
        pinchScale *= factor
        if (!pinchScale.isFinite()) return true
        val focusX = detector.focusX / density
        val focusY = detector.focusY / density
        if (!pinching && kotlin.math.abs(pinchScale - 1f) > PINCH_THRESHOLD) {
          pinching = true
          if (scrollActive) finishScroll(startMomentumAfter = false, report = false)
          zoom.onPinch(pinchScale, focusX, focusY, ZoomController.PHASE_BEGIN)
        } else if (pinching) {
          zoom.onPinch(pinchScale, focusX, focusY, ZoomController.PHASE_CHANGED)
        }
        return true
      }

      override fun onScaleEnd(detector: ScaleGestureDetector) {
        if (pinching) {
          zoom.onPinch(
            pinchScale,
            detector.focusX / density,
            detector.focusY / density,
            ZoomController.PHASE_END,
          )
        }
        pinching = false
        pinchScale = 1f
      }
    },
  )

  private val momentumFrame = Choreographer.FrameCallback { frameNanos ->
    momentumFramePosted = false
    if (!momentum.isActive) return@FrameCallback
    val dt = if (lastMomentumNanos == 0L) 1.0 / 60.0
    else (frameNanos - lastMomentumNanos).coerceAtLeast(1L) / 1_000_000_000.0
    lastMomentumNanos = frameNanos
    val step = momentum.step(dt)
    if (step.phase != PHASE_NONE) sendScroll(step.dx, step.dy, PHASE_NONE, step.phase)
    if (step.finished) lastMomentumNanos = 0L else postMomentumFrame()
  }

  override fun onTouch(view: RemoteSessionView, event: MotionEvent): Boolean {
    if (!canInteract()) return true
    input.setSession(view.sessionLong)
    velocityTracker.addMovement(event)
    when (event.actionMasked) {
      MotionEvent.ACTION_DOWN,
      MotionEvent.ACTION_POINTER_DOWN,
      MotionEvent.ACTION_MOVE -> updateBeforeDetectors(event)
    }
    scaleDetector.onTouchEvent(event)
    when (event.actionMasked) {
      MotionEvent.ACTION_POINTER_UP,
      MotionEvent.ACTION_UP,
      MotionEvent.ACTION_CANCEL -> updateAfterDetectors(event)
    }
    return true
  }

  override fun onModeChanged(mode: String) {
    if (mode == "touch") {
      cursor = controller.cursorPosition
    } else {
      cancel()
    }
  }

  override fun onCursorEvent(
    kind: Int,
    visible: Boolean,
    x: Float,
    y: Float,
    shapeId: Long,
    hotX: Int,
    hotY: Int,
    scale: Float,
    png: ByteArray?,
  ) {
    // SessionCursorController applies host arbitration before this callback.
    // Local direct-touch state is updated only by moveCursorTo().
  }

  override fun onContentRectChanged(rect: RectF) {
    contentRectDp = GRect(
      rect.left / density,
      rect.top / density,
      rect.width() / density,
      rect.height() / density,
    )
    zoom.onContentRectChanged()
  }

  override fun onDetached() {
    cancel()
    magnifier.close()
    input.close()
    velocityTracker.recycle()
  }

  override fun onCommand(name: String, args: com.facebook.react.bridge.ReadableArray?) {
    when (name) {
      "zoomReset" -> zoom.zoomReset()
      "showMagnifier" -> {
        val on = args?.takeIf { it.size() > 0 }?.getBoolean(0) ?: false
        if (on) {
          cursor = controller.cursorPosition
          magnifier.showAt(currentFinger())
        } else {
          magnifier.hide()
        }
      }
    }
  }

  override fun onKeyboardInset(px: Float) {
    zoom.keyboardInset(px)
  }

  override fun onDraw(canvas: Canvas) {
    magnifier.onDraw(canvas)
  }

  fun cancel() {
    cancelLongPress()
    zoom.cancel()
    if (leftDown) {
      input.button(BUTTON_LEFT, false, cursor)
      leftDown = false
    }
    if (scrollActive) finishScroll(startMomentumAfter = false, report = true)
    stopMomentum()
    magnifier.hide()
    input.releaseAll()
    resetState()
  }

  private fun updateBeforeDetectors(event: MotionEvent) {
    when (event.actionMasked) {
      MotionEvent.ACTION_DOWN -> {
        stopMomentum()
        resetState()
        val id = event.getPointerId(0)
        val point = eventPoint(event, 0)
        pointers[id] = point
        downPointers[id] = point
        finger = point
        processTapResults(taps.down(id, point.x, point.y, event.eventTime))
        moved = false
        longPressed = false
        longCancelled = false
        moveCursor(point)
        scheduleLongPress()
      }

      MotionEvent.ACTION_POINTER_DOWN -> {
        cancelLongPress()
        val index = event.actionIndex
        val id = event.getPointerId(index)
        val point = eventPoint(event, index)
        pointers[id] = point
        downPointers[id] = point
        processTapResults(taps.down(id, point.x, point.y, event.eventTime))
        if (pointers.size >= 2) {
          multiFinger = true
          twoFingerTapCandidate = pointers.size == 2
          if (leftDown) {
            input.button(BUTTON_LEFT, false, cursor)
            leftDown = false
          }
          if (pointers.size == 2) {
            twoFingerAnchor = centroid()
            lastCentroid = twoFingerAnchor
            lastCentroidTimeMs = event.eventTime
            lastScrollVelocity = GPoint.ZERO
          } else {
            twoFingerTapCandidate = false
          }
        }
      }

      MotionEvent.ACTION_MOVE -> {
        for (index in 0 until event.pointerCount) {
          val id = event.getPointerId(index)
          val point = eventPoint(event, index)
          pointers[id] = point
          val down = downPointers[id]
          if (down != null && hypot((point.x - down.x).toDouble(), (point.y - down.y).toDouble()) > TOUCH_SLOP_DP) {
            moved = true
            twoFingerTapCandidate = false
            cancelLongPress()
          }
          processTapResults(taps.move(id, point.x, point.y, event.eventTime))
        }
        if (pointers.size == 1 && !multiFinger) {
          val point = pointers.values.firstOrNull() ?: return
          finger = point
          if (longPressed) {
            if (!isInside(point)) {
              longCancelled = true
              magnifier.hide()
            }
            moveCursor(point)
            if (!longCancelled) magnifier.showAt(point)
          } else {
            moveCursor(point)
            if (moved && !leftDown) {
              input.button(BUTTON_LEFT, true, cursor)
              leftDown = true
            }
          }
        }
        if (pointers.size >= 2 && !pinching) updateScroll(event.eventTime)
      }
    }
  }

  private fun updateAfterDetectors(event: MotionEvent) {
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
        val point = eventPoint(event, 0)
        finger = point
        processTapResults(taps.up(id, event.eventTime))
        if (longPressed) {
          cancelLongPress()
          if (!longCancelled && isInside(point)) {
            moveCursor(point)
            input.click(BUTTON_LEFT, cursor)
          }
          magnifier.hide()
          emitGesture("magnifier")
        } else if (!multiFinger) {
          if (moved) {
            if (leftDown) input.button(BUTTON_LEFT, false, cursor)
            leftDown = false
            emitGesture("drag")
          } else {
            val count = clickCount()
            input.click(BUTTON_LEFT, cursor, count)
            emitGesture(if (count >= 2) "doubleClick" else "click")
          }
        }
        pointers.clear()
        downPointers.clear()
        if (scrollActive) finishScroll(startMomentumAfter = true, report = true)
        if (multiFinger && twoFingerTapCandidate) scheduleTapTick()
        resetSingleFlags()
      }

      MotionEvent.ACTION_CANCEL -> {
        processTapResults(emptyList())
        cancel()
      }
    }
  }

  private var lastClickCount = 1

  private fun clickCount(): Int {
    lastClickCount = clicks.register(cursor, nowSeconds())
    return lastClickCount
  }

  private fun updateScroll(eventTimeMs: Long) {
    val current = centroid()
    val dtMs = (eventTimeMs - lastCentroidTimeMs).coerceAtLeast(1L)
    val delta = GPoint(current.x - lastCentroid.x, current.y - lastCentroid.y)
    if (!scrollActive && hypot(
        (current.x - twoFingerAnchor.x).toDouble(),
        (current.y - twoFingerAnchor.y).toDouble(),
      ) > TOUCH_SLOP_DP
    ) {
      scrollActive = true
      sendScroll(0f, 0f, PHASE_BEGIN)
    }
    if (scrollActive && !pinching) {
      lastScrollVelocity = GPoint(delta.x * 1_000f / dtMs, delta.y * 1_000f / dtMs)
      sendScroll(delta.x * 2f, delta.y * 2f, PHASE_CHANGED)
    }
    lastCentroid = current
    lastCentroidTimeMs = eventTimeMs
  }

  private fun scheduleLongPress() {
    cancelLongPress()
    val runnable = Runnable {
      longPressRunnable = null
      if (pointers.size != 1 || moved || multiFinger || longPressed) return@Runnable
      longPressed = true
      longCancelled = false
      if (leftDown) {
        input.button(BUTTON_LEFT, false, cursor)
        leftDown = false
      }
      if (view.haptics) view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
      magnifier.showAt(finger)
    }
    longPressRunnable = runnable
    handler.postDelayed(runnable, ViewConfiguration.getLongPressTimeout().toLong())
  }

  private fun cancelLongPress() {
    longPressRunnable?.let(handler::removeCallbacks)
    longPressRunnable = null
  }

  private fun processTapResults(results: List<TapResult>) {
    results.forEach { result ->
      when (result) {
        is TapResult.TwoFingerTap -> {
          val point = mapPoint(GPoint(result.cx, result.cy))
          moveCursorTo(point)
          input.click(BUTTON_RIGHT, point)
          emitGesture("rightClick")
        }
        TapResult.TwoFingerDoubleTap -> emitGesture("twoFingerDoubleTap")
        TapResult.ThreeFingerTap -> emitGesture("threeFingerTap")
      }
    }
  }

  private fun scheduleTapTick() {
    val deadline = SystemClock.uptimeMillis() + DOUBLE_TAP_WINDOW_MS + TAP_TICK_INTERVAL_MS
    val runnable = object : Runnable {
      override fun run() {
        val now = SystemClock.uptimeMillis()
        processTapResults(taps.tick(now))
        if (now < deadline) handler.postDelayed(this, TAP_TICK_INTERVAL_MS)
      }
    }
    handler.postDelayed(runnable, TAP_TICK_INTERVAL_MS)
  }

  private fun moveCursor(viewPoint: GPoint) {
    finger = viewPoint
    moveCursorTo(mapPoint(viewPoint))
    input.pointer(cursor)
    if (longPressed && !longCancelled) magnifier.setCursor(cursor, viewPoint)
  }

  private fun moveCursorTo(point: GPoint) {
    cursor = GPoint(point.x.coerceIn(0f, 1f), point.y.coerceIn(0f, 1f))
    magnifier.setCursor(cursor, finger)
    // The controller owns the R4B cursor drawing and host/local arbitration.
    controller.moveCursorLocal(cursor)
  }

  private fun mapPoint(point: GPoint): GPoint =
    TouchTapMapper.normalized(point, contentRectDp, Viewport(controller.viewport.zoom, controller.viewport.offset))

  private fun currentFinger(): GPoint = controller.viewport.viewPointFromNormalized(cursor)

  private fun finishScroll(startMomentumAfter: Boolean, report: Boolean) {
    if (!scrollActive) return
    scrollActive = false
    sendScroll(0f, 0f, PHASE_END)
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
    if (wasActive) sendScroll(0f, 0f, PHASE_NONE, PHASE_END)
  }

  private fun sendScroll(dx: Float, dy: Float, phase: Int, momentumPhase: Int = PHASE_NONE) {
    val sign = if (view.naturalScroll) 1f else -1f
    input.scroll(dx * sign, dy * sign, phase, momentumPhase)
  }

  private fun centroid(): GPoint {
    val first = pointers.values.take(2)
    if (first.isEmpty()) return GPoint.ZERO
    return GPoint(
      first.sumOf { it.x.toDouble() }.toFloat() / first.size,
      first.sumOf { it.y.toDouble() }.toFloat() / first.size,
    )
  }

  private fun eventPoint(event: MotionEvent, index: Int): GPoint =
    GPoint(event.getX(index) / density, event.getY(index) / density)

  private fun isInside(point: GPoint): Boolean =
    point.x >= 0f && point.y >= 0f && point.x <= view.width / density && point.y <= view.height / density

  private fun canInteract(): Boolean = view.sessionLong != null && view.isShown

  private fun resetState() {
    pointers.clear()
    downPointers.clear()
    taps = MultiFingerTaps()
    clicks = ClickCounter()
    resetSingleFlags()
    multiFinger = false
    twoFingerTapCandidate = false
    pinching = false
    pinchScale = 1f
    scrollActive = false
    lastScrollVelocity = GPoint.ZERO
  }

  private fun resetSingleFlags() {
    moved = false
    longPressed = false
    longCancelled = false
    leftDown = false
  }

  private fun nowSeconds(): Double = System.nanoTime() / 1_000_000_000.0

  private fun emitGesture(kind: String) = view.emitGesture(kind)

  companion object {
    private const val BUTTON_LEFT = 0
    private const val BUTTON_RIGHT = 1
    private const val PINCH_THRESHOLD = 0.05f
    private const val TOUCH_SLOP_DP = 10f
    private const val TAP_TICK_INTERVAL_MS = 50L
    private const val DOUBLE_TAP_WINDOW_MS = 300L
    private const val PHASE_NONE = 0
    private const val PHASE_BEGIN = 1
    private const val PHASE_CHANGED = 2
    private const val PHASE_END = 3
  }
}
