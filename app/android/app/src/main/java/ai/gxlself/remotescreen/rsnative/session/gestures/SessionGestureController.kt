package ai.gxlself.remotescreen.rsnative.session.gestures

import android.graphics.Canvas
import android.graphics.RectF
import android.view.MotionEvent
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.GestureLifecycle
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.logic.gestures.SpaceSwipe
import ai.gxlself.remotescreen.logic.gestures.SpaceSwipeDirection
import ai.gxlself.remotescreen.logic.gestures.SpaceSwipeTracker
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import ai.gxlself.remotescreen.rsnative.session.zoom.TouchModeGestures
import com.facebook.react.bridge.ReadableArray

/** Optional extension owned by the direct-touch/zoom task. */
interface ZoomHandler {
  fun onPinch(scale: Float, focusXDp: Float, focusYDp: Float, phase: Int)
  fun onCursorMoved(cursor: GPoint)
  val viewport: SessionViewport
}

/** Connects the Android view lifecycle to the trackpad and future touch modes. */
class SessionGestureController(
  private val view: RemoteSessionView,
) : RemoteSessionView.SessionGestureLayer, RsActivityHooks.Listener {
  /** Exposed for the zoom installer; gesture ownership remains in this controller. */
  val sessionView: RemoteSessionView
    get() = view

  /** Updates the shared cursor model for a local direct-touch sample. */
  fun moveCursorLocal(point: GPoint) {
    if (lifecycle.state == GestureLifecycle.State.Closed) return
    cursor.moveTo(point)
  }

  val cursorPosition: GPoint
    get() = cursor.position

  private val lifecycle = GestureLifecycle()
  private val density = view.resources.displayMetrics.density.takeIf { it.isFinite() && it > 0f } ?: 1f
  private val baseViewport = SessionViewport()
  private val input = SessionInputSender { view.sessionLong }
  private val cursor = SessionCursorController(view, { viewport }) { point ->
    zoomHandler?.onCursorMoved(point)
  }
  private val trackpad = TrackpadGestures(
    view = view,
    input = input,
    cursor = cursor,
    zoomHandlerProvider = { zoomHandler },
    emitGesture = view::emitGesture,
    cursorSpeedProvider = { view.cursorSpeed },
    naturalScrollProvider = { view.naturalScroll },
    hapticsProvider = { view.haptics },
    sessionReadyProvider = { view.sessionLong != null },
  )
  private val spaceSwipe = SpaceSwipeTracker()
  private var exclusionActive: Boolean? = null
  private var exclusionWidth = -1
  private var exclusionHeight = -1
  private var exclusionDensity = Float.NaN

  var touchMode: RemoteSessionView.SessionGestureLayer? = null
    set(value) {
      field = value
      if (lifecycle.state == GestureLifecycle.State.Active) value?.onWindowAttached()
    }
  var zoomHandler: ZoomHandler? = null
  var mode: String = "trackpad"
    private set

  val actualLongPressMs: Long
    get() = trackpad.actualLongPressMs

  val viewport: SessionViewport
    get() = zoomHandler?.viewport ?: baseViewport

  init {
    if (view.isAttachedToWindow) applyLifecycle(lifecycle.windowAttached())
  }

  override fun onTouch(view: RemoteSessionView, event: MotionEvent): Boolean {
    if (lifecycle.state != GestureLifecycle.State.Active) return true
    if (handleSpaceSwipe(event)) return true
    input.setSession(view.sessionLong)
    if (mode == "touch" && touchMode != null) return touchMode?.onTouch(view, event) == true
    return trackpad.onTouch(event)
  }

  override fun onModeChanged(mode: String) {
    val next = if (mode == "touch") "touch" else "trackpad"
    if (lifecycle.state == GestureLifecycle.State.Closed) return
    if (this.mode == next) return
    if (lifecycle.state == GestureLifecycle.State.Active) {
      input.setSession(view.sessionLong)
      trackpad.cancel()
      input.releaseAll()
    }
    this.mode = next
    touchMode?.onModeChanged(next)
    if (next == "touch" && touchMode == null) {
      RsLog.marker("SESSVIEW", mapOf("mode" to "touch", "impl" to false))
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
    if (lifecycle.state == GestureLifecycle.State.Closed) return
    if (lifecycle.state == GestureLifecycle.State.Active) input.setSession(view.sessionLong)
    cursor.onCursorEvent(kind, visible, x, y, shapeId, hotX, hotY, scale, png)
    touchMode?.onCursorEvent(kind, visible, x, y, shapeId, hotX, hotY, scale, png)
    if (lifecycle.state == GestureLifecycle.State.Active) updateEdgeExclusionIfChanged()
  }

  override fun onContentRectChanged(rect: RectF) {
    if (lifecycle.state == GestureLifecycle.State.Closed) return
    baseViewport.contentRect = GRect(
      rect.left / density,
      rect.top / density,
      rect.width() / density,
      rect.height() / density,
    )
    baseViewport.zoom = 1f
    touchMode?.onContentRectChanged(rect)
    if (lifecycle.state == GestureLifecycle.State.Active) updateEdgeExclusionIfChanged()
    view.invalidateOverlay()
  }

  override fun onWindowAttached() {
    applyLifecycle(lifecycle.windowAttached())
  }

  override fun onWindowDetached() {
    applyLifecycle(lifecycle.windowDetached())
  }

  override fun onDetached() {
    applyLifecycle(lifecycle.drop())
  }

  override fun onCommand(name: String, args: ReadableArray?) {
    if (lifecycle.state != GestureLifecycle.State.Active) return
    when (name) {
      "centerCursor" -> {
        input.setSession(view.sessionLong)
        cursor.center()
        input.centerCursor()
      }
      "zoomReset", "showMagnifier" -> {
        val handler = touchMode
        if (handler == null) {
          RsLog.marker("SESSVIEW", mapOf("command" to name, "impl" to false))
        } else {
          handler.onCommand(name, args)
        }
      }
      else -> RsLog.marker("SESSVIEW", mapOf("command" to name, "impl" to false))
    }
  }

  override fun onKeyboardInset(px: Float) {
    if (lifecycle.state != GestureLifecycle.State.Active) return
    touchMode?.onKeyboardInset(px)
  }

  override fun onDraw(canvas: Canvas) {
    if (lifecycle.state == GestureLifecycle.State.Closed) return
    touchMode?.onDraw(canvas)
    cursor.draw(canvas)
  }

  override fun onUserLeaveHint(activity: android.app.Activity) {
    cancelAll()
  }

  fun onWindowFocusChanged(hasFocus: Boolean) {
    if (!hasFocus) cancelAll()
  }

  private fun cancelAll() {
    if (lifecycle.state != GestureLifecycle.State.Active) return
    trackpad.cancel()
    cancelTouchModeForWindow()
    input.releaseAll()
    input.setSession(null)
  }

  private fun applyLifecycle(actions: GestureLifecycle.Actions) {
    if (actions.cancelGestures) {
      trackpad.cancel()
      cancelTouchModeForWindow()
    }
    if (actions.releaseInputs) {
      input.releaseAll()
      input.setSession(null)
    }
    if (actions.clearEdgeExclusion) {
      EdgeExclusion.clear(view)
      exclusionActive = null
      exclusionWidth = -1
      exclusionHeight = -1
      exclusionDensity = Float.NaN
    }
    if (actions.unregisterHooks) RsActivityHooks.remove(this)
    if (actions.registerHooks) RsActivityHooks.add(this)
    if (actions.bindSession) {
      input.setSession(view.sessionLong)
      touchMode?.onWindowAttached()
    }
    if (actions.restoreEdgeExclusion) updateEdgeExclusionIfChanged()
    if (actions.closeResources) {
      trackpad.close()
      touchMode?.onDetached()
      input.close()
      cursor.reset()
      cursor.close()
    }
  }

  /**
   * three fingers moving sideways switch computers. Once it is a
   * swipe the touch is ours: whatever the remote got is cancelled, and on
   * release the page is told to switch (no follow-the-finger slide here).
   */
  private fun handleSpaceSwipe(event: MotionEvent): Boolean {
    val action = event.actionMasked
    if (action == MotionEvent.ACTION_DOWN) spaceSwipe.reset()
    if (action == MotionEvent.ACTION_CANCEL) {
      val was = spaceSwipe.swiping
      spaceSwipe.reset()
      if (was) view.emitSpaceSwipe("ended")
      return was
    }
    val lifting = action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_POINTER_UP
    if (lifting && spaceSwipe.swiping) {
      val (previous, next) = SpaceSwipe.neighbours(view.sessionKey, view.spaceIds)
      val direction = spaceSwipe.release(view.width / density, previous != null, next != null)
      view.emitSpaceSwipe("ended")
      val target = when (direction) {
        SpaceSwipeDirection.Previous -> previous
        SpaceSwipeDirection.Next -> next
        null -> null
      }
      if (target != null) view.emitSpaceSwitch(target)
      if (action == MotionEvent.ACTION_UP) spaceSwipe.reset()
      return true
    }
    var count = 0
    var x = 0f
    var y = 0f
    for (index in 0 until event.pointerCount) {
      if (lifting && index == event.actionIndex) continue
      count++
      x += event.getX(index)
      y += event.getY(index)
    }
    val step = if (count == 0) {
      spaceSwipe.track(0, 0f, 0f, event.eventTime)
    } else {
      spaceSwipe.track(count, x / count / density, y / count / density, event.eventTime)
    }
    if (step == SpaceSwipeTracker.Step.Began) {
      trackpad.cancel()
      cancelTouchModeForWindow()
      input.releaseAll()
      view.emitSpaceSwipe("began")
      RsLog.marker("SPACES", mapOf("swipe" to "began", "count" to view.spaceIds.size))
    }
    if (action == MotionEvent.ACTION_UP) spaceSwipe.reset()
    return step != SpaceSwipeTracker.Step.Pass
  }

  private fun cancelTouchModeForWindow() {
    when (val layer = touchMode) {
      is TouchModeGestures -> layer.cancel()
      null -> Unit
      else -> layer.onWindowDetached()
    }
  }

  private fun updateEdgeExclusionIfChanged() {
    if (lifecycle.state != GestureLifecycle.State.Active) return
    val densityNow = view.resources.displayMetrics.density
      .takeIf { it.isFinite() && it > 0f } ?: 1f
    val activeNow = view.sessionLong != null && view.width > 0 && view.height > 0
    if (
      exclusionActive == activeNow &&
      exclusionWidth == view.width &&
      exclusionHeight == view.height &&
      exclusionDensity == densityNow
    ) {
      return
    }
    exclusionActive = activeNow
    exclusionWidth = view.width
    exclusionHeight = view.height
    exclusionDensity = densityNow
    EdgeExclusion.update(view)
  }
}
