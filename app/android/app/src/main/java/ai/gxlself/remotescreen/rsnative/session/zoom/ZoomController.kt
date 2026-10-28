package ai.gxlself.remotescreen.rsnative.session.zoom

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.ValueAnimator
import android.os.SystemClock
import android.view.animation.DecelerateInterpolator
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.logic.zoom.Viewport
import ai.gxlself.remotescreen.logic.zoom.ViewportEmitter
import ai.gxlself.remotescreen.logic.zoom.ZoomMath
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import ai.gxlself.remotescreen.rsnative.session.gestures.ZoomHandler
import kotlin.math.abs

/** Owns local viewport state shared by trackpad and direct-touch pinch gestures. */
class ZoomController(
  private val view: RemoteSessionView,
) : ZoomHandler {
  private val density = view.resources.displayMetrics.density
    .takeIf { it.isFinite() && it > 0f } ?: 1f
  private val state = SessionViewport()
  private val emitter = ViewportEmitter()
  private var animator: ValueAnimator? = null
  private var pinchActive = false
  private var lastPinchScale = 1f
  private var lastPinchFocus = GPoint.ZERO
  private var lastCursor = GPoint(0.5f, 0.5f)
  private var savedKeyboardViewport: Viewport? = null
  private var reportedInteracting = false

  init {
    view.bindZoomController(this)
  }

  val interacting: Boolean
    get() = pinchActive || animator != null

  override val viewport: SessionViewport
    get() {
      syncContentRect()
      return state
    }

  override fun onPinch(scale: Float, focusXDp: Float, focusYDp: Float, phase: Int) {
    syncContentRect()
    val focus = GPoint(focusXDp, focusYDp)
    val safeScale = scale.takeIf { it.isFinite() && it > 0f } ?: lastPinchScale
    if (phase == PHASE_BEGIN || !pinchActive) {
      cancelAnimation()
      pinchActive = true
      notifyInteractionChanged()
      lastPinchScale = 1f
      lastPinchFocus = focus
    }

    val factor = (safeScale / lastPinchScale).takeIf { it.isFinite() && it > 0f } ?: 1f
    val current = pureViewport()
    val anchor = state.normalizedFromView(lastPinchFocus)
    var next = ZoomMath.zoomed(current, factor, anchor)
    val rect = state.contentRect
    if (rect.w > 0f && rect.h > 0f && next.zoom > 0f) {
      next = ZoomMath.panned(
        next,
        GPoint(
          (focus.x - lastPinchFocus.x) / (rect.w * next.zoom),
          (focus.y - lastPinchFocus.y) / (rect.h * next.zoom),
        ),
      )
    }
    apply(next, emit = true)
    lastPinchScale = safeScale
    lastPinchFocus = focus

    if (phase == PHASE_END) {
      pinchActive = false
      notifyInteractionChanged()
      flushViewport()
      view.emitGesture("zoom")
    }
  }

  override fun onCursorMoved(cursor: GPoint) {
    if (!cursor.x.isFinite() || !cursor.y.isFinite()) return
    lastCursor = GPoint(cursor.x.coerceIn(0f, 1f), cursor.y.coerceIn(0f, 1f))
    syncContentRect()
    val next = ZoomMath.followCursor(pureViewport(), lastCursor)
    apply(next, emit = true)
  }

  fun zoomReset() {
    syncContentRect()
    val target = Viewport(ZoomMath.MIN_ZOOM, GPoint.ZERO)
    animateTo(target, RESET_DURATION_MS)
  }

  fun keyboardInset(px: Float) {
    view.updateKeyboardInset(px)
    syncContentRect()
    val heightPx = view.contentRect.height().takeIf { it.isFinite() && it > 0f } ?: return
    val insetPx = px.takeIf { it.isFinite() } ?: 0f
    if (insetPx <= 0f) {
      val saved = savedKeyboardViewport ?: return
      savedKeyboardViewport = null
      animateTo(saved, KEYBOARD_DURATION_MS)
      return
    }
    if (savedKeyboardViewport == null) savedKeyboardViewport = pureViewport()
    val fraction = (insetPx / heightPx).coerceIn(0f, 0.99f)
    val target = ZoomMath.keyboardTarget(pureViewport(), lastCursor, fraction)
    animateTo(target, KEYBOARD_DURATION_MS)
  }

  fun onContentRectChanged() {
    syncContentRect()
  }

  fun cancel() {
    cancelAnimation()
    pinchActive = false
    notifyInteractionChanged()
    flushViewport()
  }

  private fun animateTo(target: Viewport, durationMs: Long) {
    cancelAnimation()
    syncContentRect()
    val start = pureViewport()
    if (view.reduceMotion || same(start, target)) {
      apply(target, emit = true)
      flushViewport()
      return
    }
    val animation = ValueAnimator.ofFloat(0f, 1f).apply {
      duration = durationMs
      interpolator = DecelerateInterpolator()
      addUpdateListener { valueAnimator ->
        val t = (valueAnimator.animatedValue as? Float) ?: return@addUpdateListener
        val value = Viewport(
          start.zoom + (target.zoom - start.zoom) * t,
          GPoint(
            start.offset.x + (target.offset.x - start.offset.x) * t,
            start.offset.y + (target.offset.y - start.offset.y) * t,
          ),
        )
        apply(value, emit = true)
      }
      addListener(object : AnimatorListenerAdapter() {
        override fun onAnimationEnd(animation: Animator) {
          if (animator === animation) animator = null
          notifyInteractionChanged()
          apply(target, emit = true)
          flushViewport()
        }

        override fun onAnimationCancel(animation: Animator) {
          if (animator === animation) animator = null
          notifyInteractionChanged()
        }
      })
    }
    animator = animation
    notifyInteractionChanged()
    animation.start()
  }

  private fun apply(value: Viewport, emit: Boolean) {
    val nextZoom = value.zoom.takeIf { it.isFinite() }?.coerceIn(ZoomMath.MIN_ZOOM, ZoomMath.MAX_ZOOM)
      ?: ZoomMath.MIN_ZOOM
    val nextOffset = ZoomMath.clampOffset(nextZoom, value.offset)
    val changed = abs(state.zoom - nextZoom) > EPSILON || state.offset != nextOffset
    state.zoom = nextZoom
    state.offset = nextOffset
    if (!changed) return
    view.applyViewport(state.zoom, state.offset.x, state.offset.y)
    view.onZoomViewportChanged()
    view.invalidateOverlay()
    if (emit) {
      emitter.offer(Viewport(state.zoom, state.offset), SystemClock.uptimeMillis())?.let(::emit)
    }
  }

  private fun flushViewport() {
    emitter.flush()?.let(::emit)
  }

  private fun emit(value: Viewport) {
    view.emitViewport(value.zoom, value.offset.x, value.offset.y)
  }

  private fun pureViewport(): Viewport = Viewport(state.zoom, state.offset)

  private fun same(a: Viewport, b: Viewport): Boolean =
    abs(a.zoom - b.zoom) <= EPSILON && a.offset == b.offset

  private fun syncContentRect() {
    val rect = view.contentRect
    if (rect.width() <= 0f || rect.height() <= 0f) return
    state.contentRect = GRect(
      rect.left / density,
      rect.top / density,
      rect.width() / density,
      rect.height() / density,
    )
  }

  private fun cancelAnimation() {
    animator?.cancel()
    animator = null
    notifyInteractionChanged()
  }

  private fun notifyInteractionChanged() {
    val next = interacting
    if (next == reportedInteracting) return
    reportedInteracting = next
    view.onZoomInteractionChanged()
  }

  companion object {
    const val PHASE_BEGIN = 1
    const val PHASE_CHANGED = 2
    const val PHASE_END = 3
    private const val RESET_DURATION_MS = 200L
    private const val KEYBOARD_DURATION_MS = 250L
    private const val EPSILON = 0.000001f
  }
}
