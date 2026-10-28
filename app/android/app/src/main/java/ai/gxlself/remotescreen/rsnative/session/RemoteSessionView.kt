package ai.gxlself.remotescreen.rsnative.session

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.view.PixelCopy
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageView
import androidx.core.graphics.createBitmap
import ai.gxlself.remotescreen.logic.session.ContentRect
import ai.gxlself.remotescreen.logic.session.VideoStatsSnapshot
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.logic.viewport.ViewportGeometry
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import ai.gxlself.remotescreen.rsnative.jni.RsSessionNative
import ai.gxlself.remotescreen.rsnative.session.video.SessionEventSink
import ai.gxlself.remotescreen.rsnative.session.video.SessionVideoPipeline
import com.facebook.react.bridge.ReadableArray
import kotlin.math.roundToInt

/** Fabric-hosted video surface for one remote session. */
@SuppressLint("ClickableViewAccessibility")
class RemoteSessionView(context: Context) : FrameLayout(context), SessionEventSink,
  SurfaceHolder.Callback, SessionVideoPipeline.GeometrySink {
  interface SessionGestureLayer {
    fun onTouch(view: RemoteSessionView, event: MotionEvent): Boolean
    fun onModeChanged(mode: String) {}
    fun onCursorEvent(
      kind: Int,
      visible: Boolean,
      x: Float,
      y: Float,
      shapeId: Long,
      hotX: Int,
      hotY: Int,
      scale: Float,
      png: ByteArray?,
    ) {}
    fun onContentRectChanged(rect: RectF) {}
    fun onWindowAttached() {}
    fun onWindowDetached() {}
    fun onDetached() {}
    fun onCommand(name: String, args: ReadableArray?) {}
    fun onKeyboardInset(px: Float) {}
    fun onDraw(canvas: Canvas) {}
  }

  private class OverlayView(context: Context, private val owner: RemoteSessionView) : View(context) {
    init {
      setWillNotDraw(false)
      setBackgroundColor(Color.TRANSPARENT)
      importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    }

    override fun onDraw(canvas: Canvas) {
      super.onDraw(canvas)
      owner.gestureLayer?.onDraw(canvas)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean = false
  }

  val surfaceView: SurfaceView = SurfaceView(context)
  private val backdropView: ImageView = ImageView(context)
  val overlay: View = OverlayView(context, this)
  val contentRect: RectF = RectF()

  var gestureLayer: SessionGestureLayer? = null
    set(value) {
      field = value
      overlay.invalidate()
    }

  var sessionLong: Long? = null
    set(value) {
      field = value
      reportViewport()
    }

  var toolbarCollapsed: Boolean = false
  var reduceMotion: Boolean = false
  var cursorSpeed: Float = 1.4f
  var naturalScroll: Boolean = true
  var haptics: Boolean = true
  /** open computers in swipe order, and their names. */
  var spaceIds: List<String> = emptyList()
  var spaceNames: List<String> = emptyList()

  private val pipeline = SessionVideoPipeline(this)
  private val attachment = SessionAttachment(pipelineFactory = { pipeline }) { id ->
    sessionLong = id
  }
  private var sessionId: String = ""
  private var videoWidth = 0
  private var videoHeight = 0
  private var viewportZoom = 1f
  private var viewportOffsetX = 0f
  private var viewportOffsetY = 0f
  private var zoomController: ai.gxlself.remotescreen.rsnative.session.zoom.ZoomController? = null
  private var keyboardInsetPx = 0f
  private var viewportCropAllowed = true
  private var streamSource = GRect(0f, 0f, 1f, 1f)
  private var streamEpoch = 0
  private var backdropBitmap: Bitmap? = null
  private var captureGeneration = 0L
  private val mainHandler = Handler(Looper.getMainLooper())

  init {
    setBackgroundColor(Color.BLACK)
    importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES

    surfaceView.setBackgroundColor(Color.BLACK)
    surfaceView.setZOrderMediaOverlay(false)
    surfaceView.importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    surfaceView.holder.addCallback(this)
    backdropView.setBackgroundColor(Color.TRANSPARENT)
    backdropView.scaleType = ImageView.ScaleType.FIT_XY
    addView(backdropView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    addView(surfaceView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))

    overlay.setOnTouchListener { _, event -> gestureLayer?.onTouch(this, event) ?: false }
    addView(overlay, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
  }

  fun setSessionId(value: String?) {
    val next = value.orEmpty()
    if (sessionId == next) return
    sessionId = next
    videoWidth = 0
    videoHeight = 0
    clearBackdrop()
    streamSource = GRect(0f, 0f, 1f, 1f)
    streamEpoch = 0
    requestLayout()
    if (isAttachedToWindow) {
      attachment.attach(next)
    } else if (next.isEmpty()) {
      attachment.detach()
    }
  }

  fun setMode(value: String?) {
    gestureLayer?.onModeChanged(if (value == "touch") "touch" else "trackpad")
  }

  fun emitGesture(kind: String) {
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putString("kind", kind)
    RsViewEvents.emit(this, "topGesture", event)
  }

  /** The session shown, as JS names it ("" when detached). */
  val sessionKey: String
    get() = sessionId

  /** a three-finger swipe began / ended. */
  fun emitSpaceSwipe(phase: String) {
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putString("phase", phase)
    RsViewEvents.emit(this, "topSpaceSwipe", event)
  }

  /** switch to [target] now. */
  fun emitSpaceSwitch(target: String) {
    RsLog.marker("SPACES", mapOf("switch" to true))
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putString("sessionId", target)
    RsViewEvents.emit(this, "topSpaceSwitch", event)
  }

  fun emitViewport(zoom: Float, x: Float, y: Float) {
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putDouble("zoom", zoom.toDouble())
    event.putDouble("x", x.toDouble())
    event.putDouble("y", y.toDouble())
    RsViewEvents.emit(this, "topViewport", event)
  }

  fun invalidateOverlay() {
    overlay.invalidate()
  }

  fun applyViewport(zoom: Float, offsetX: Float, offsetY: Float) {
    viewportZoom = zoom.takeIf { it.isFinite() && it > 0f } ?: 1f
    viewportOffsetX = offsetX.takeIf { it.isFinite() } ?: 0f
    viewportOffsetY = offsetY.takeIf { it.isFinite() } ?: 0f
    layoutSurface()
  }

  internal fun bindZoomController(controller: ai.gxlself.remotescreen.rsnative.session.zoom.ZoomController) {
    zoomController = controller
    reportViewport()
  }

  internal fun onZoomViewportChanged() = reportViewport()

  internal fun onZoomInteractionChanged() = reportViewport()

  internal fun updateKeyboardInset(px: Float) {
    keyboardInsetPx = px.takeIf { it.isFinite() }?.coerceAtLeast(0f) ?: 0f
    reportViewport()
  }

  fun setViewportCropAllowed(allowed: Boolean) {
    if (viewportCropAllowed == allowed) return
    viewportCropAllowed = allowed
    reportViewport()
  }

  fun reportViewport() {
    val id = sessionLong ?: return
    if (contentRect.width() <= 0f || contentRect.height() <= 0f || width <= 0 || height <= 0) return
    val zoom = zoomController?.viewport?.zoom ?: viewportZoom
    val offset = zoomController?.viewport?.offset ?: GPoint(viewportOffsetX, viewportOffsetY)
    val viewport = SessionViewport(
      contentRect = GRect(contentRect.left, contentRect.top, contentRect.width(), contentRect.height()),
      zoom = zoom,
      offset = offset,
    )
    val visible = ViewportGeometry.visibleRect(viewport, width.toFloat(), height.toFloat(), keyboardInsetPx)
    val pixels = ViewportGeometry.viewPixels(visible, viewport)
    val flags = (if (zoomController?.interacting == true) RsSessionNative.VIEWPORT_ACTIVE else 0) or
      (if (viewportCropAllowed) RsSessionNative.VIEWPORT_CROP_OK else 0)
    RsSessionNative.setViewport(
      id,
      visible.x,
      visible.y,
      visible.w,
      visible.h,
      pixels.width.coerceAtLeast(0),
      pixels.height.coerceAtLeast(0),
      flags,
    )
  }

  fun dispatchCommand(name: String, args: ReadableArray?) {
    val layer = gestureLayer
    if (layer == null) {
      RsLog.marker("SESSVIEW", mapOf("command" to name, "impl" to false))
    } else {
      layer.onCommand(name, args)
    }
  }

  fun detachSession() {
    attachment.detach()
    clearBackdrop()
    streamSource = GRect(0f, 0f, 1f, 1f)
    streamEpoch = 0
    requestLayout()
  }

  override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
    super.onMeasure(widthMeasureSpec, heightMeasureSpec)
    val width = measuredWidth
    val height = measuredHeight
    backdropView.measure(
      MeasureSpec.makeMeasureSpec(width, MeasureSpec.EXACTLY),
      MeasureSpec.makeMeasureSpec(height, MeasureSpec.EXACTLY),
    )
    surfaceView.measure(
      MeasureSpec.makeMeasureSpec(width, MeasureSpec.EXACTLY),
      MeasureSpec.makeMeasureSpec(height, MeasureSpec.EXACTLY),
    )
    overlay.measure(
      MeasureSpec.makeMeasureSpec(width, MeasureSpec.EXACTLY),
      MeasureSpec.makeMeasureSpec(height, MeasureSpec.EXACTLY),
    )
  }

  override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
    super.onLayout(changed, left, top, right, bottom)
    layoutSurface()
    overlay.layout(0, 0, right - left, bottom - top)
    reportViewport()
  }

  private fun layoutSurface() {
    val width = width.toFloat()
    val height = height.toFloat()
    val logical = ViewportGeometry.logicalVideoSize(videoWidth, videoHeight, streamSource)
    val sourceWidth = if (logical.width > 0) logical.width.toFloat() else 16f
    val sourceHeight = if (logical.height > 0) logical.height.toFloat() else 9f
    val fitted = ContentRect.fit(sourceWidth, sourceHeight, width, height)
    contentRect.set(fitted.x, fitted.y, fitted.x + fitted.w, fitted.y + fitted.h)
    gestureLayer?.onContentRectChanged(RectF(contentRect))

    val zoom = viewportZoom
    val x = contentRect.left - viewportOffsetX * zoom * contentRect.width()
    val y = contentRect.top - viewportOffsetY * zoom * contentRect.height()
    val surfaceWidth = contentRect.width() * zoom
    val surfaceHeight = contentRect.height() * zoom
    backdropView.layout(
      x.roundToInt(),
      y.roundToInt(),
      (x + surfaceWidth).roundToInt(),
      (y + surfaceHeight).roundToInt(),
    )
    val sourceLeft = x + streamSource.x * surfaceWidth
    val sourceTop = y + streamSource.y * surfaceHeight
    val sourceRight = sourceLeft + streamSource.w * surfaceWidth
    val sourceBottom = sourceTop + streamSource.h * surfaceHeight
    surfaceView.layout(
      sourceLeft.roundToInt(),
      sourceTop.roundToInt(),
      sourceRight.roundToInt(),
      sourceBottom.roundToInt(),
    )
  }

  override fun surfaceCreated(holder: SurfaceHolder) {
    pipeline.setSurface(holder.surface, surfaceView.width, surfaceView.height)
  }

  override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
    pipeline.setSurface(holder.surface, width, height)
  }

  override fun surfaceDestroyed(holder: SurfaceHolder) {
    pipeline.setSurface(null, 0, 0)
  }

  override fun onVideoSize(width: Int, height: Int) {
    if (width <= 0 || height <= 0) return
    videoWidth = width
    videoHeight = height
    surfaceView.holder.setFixedSize(width, height)
    requestLayout()
    reportViewport()
  }

  override fun onStreamCrop(
    epoch: Int,
    srcX: Float,
    srcY: Float,
    srcW: Float,
    srcH: Float,
    displayW: Int,
    displayH: Int,
  ) {
    val next = if (displayW == 0) {
      GRect(0f, 0f, 1f, 1f)
    } else {
      ViewportGeometry.source(srcX, srcY, srcW, srcH)
    }
    if (epoch == streamEpoch && next == streamSource) {
      reportViewport()
      return
    }
    val wasFull = ViewportGeometry.isFull(streamSource)
    val becomesCropped = !ViewportGeometry.isFull(next)
    val needsBackdrop = wasFull && becomesCropped && backdropBitmap == null
    val applyGeometry: () -> Unit = {
      streamEpoch = epoch
      streamSource = next
      if (videoWidth > 0 && videoHeight > 0) {
        surfaceView.holder.setFixedSize(videoWidth, videoHeight)
      }
      requestLayout()
      layoutSurface()
      reportViewport()
      val listener = object : android.view.ViewTreeObserver.OnPreDrawListener {
        override fun onPreDraw(): Boolean {
          if (viewTreeObserver.isAlive) viewTreeObserver.removeOnPreDrawListener(this)
          pipeline.geometryReady(epoch)
          return true
        }
      }
      viewTreeObserver.addOnPreDrawListener(listener)
    }
    if (needsBackdrop) captureBackdrop(applyGeometry) else applyGeometry()
  }

  override fun onVideoFrameRendered(epoch: Int) {
    if (epoch != streamEpoch) return
    if (ViewportGeometry.isFull(streamSource)) clearBackdrop()
  }

  override fun onFirstFrame(epoch: Int, width: Int, height: Int) {
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putInt("epoch", epoch)
    event.putInt("width", width)
    event.putInt("height", height)
    RsViewEvents.emit(this, "topFirstFrame", event)
  }

  override fun onVideoStats(stats: VideoStatsSnapshot) {
    val event = com.facebook.react.bridge.Arguments.createMap()
    event.putInt("fps", stats.fps)
    event.putDouble("decodeP50Ms", stats.decodeP50Ms.toDouble())
    event.putDouble("decodeP95Ms", stats.decodeP95Ms.toDouble())
    event.putDouble("latencyP50Ms", stats.latencyP50Ms.toDouble())
    event.putInt("queue", stats.queue)
    event.putInt("dropped", stats.dropped)
    event.putInt("keyframeRequests", stats.keyframeRequests)
    event.putInt("width", stats.width)
    event.putInt("height", stats.height)
    event.putString("codec", stats.codec)
    RsViewEvents.emit(this, "topVideoStats", event)
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
    gestureLayer?.onCursorEvent(kind, visible, x, y, shapeId, hotX, hotY, scale, png)
  }

  override fun onClosed(reason: String) {
    RsLog.marker("SESSVIEW", mapOf("closed" to true))
  }

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    if (sessionId.isNotEmpty()) attachment.attach(sessionId)
    gestureLayer?.onWindowAttached()
  }

  override fun onDetachedFromWindow() {
    gestureLayer?.onWindowDetached()
    attachment.detach()
    clearBackdrop()
    super.onDetachedFromWindow()
  }

  private fun captureBackdrop(onComplete: () -> Unit) {
    if (surfaceView.width <= 0 || surfaceView.height <= 0 || !surfaceView.holder.surface.isValid
    ) {
      RsLog.marker("SESSVIEW", mapOf("backdrop" to "unavailable"))
      onComplete()
      return
    }
    val bitmap = createBitmap(surfaceView.width, surfaceView.height, Bitmap.Config.ARGB_8888)
    val generation = ++captureGeneration
    val callback = PixelCopy.OnPixelCopyFinishedListener { result ->
      val current = generation == captureGeneration && sessionId.isNotEmpty()
      if (!current) {
        bitmap.recycle()
        return@OnPixelCopyFinishedListener
      }
      if (result == PixelCopy.SUCCESS) {
        backdropBitmap?.recycle()
        backdropBitmap = bitmap
        backdropView.setImageBitmap(bitmap)
        layoutSurface()
      } else {
        bitmap.recycle()
        RsLog.marker("SESSVIEW", mapOf("backdrop" to "pixel-copy-failed", "result" to result))
      }
      onComplete()
    }
    try {
      PixelCopy.request(surfaceView, bitmap, callback, mainHandler)
    } catch (error: RuntimeException) {
      bitmap.recycle()
      RsLog.marker("SESSVIEW", mapOf("backdrop" to "pixel-copy-failed", "type" to error.javaClass.simpleName))
      onComplete()
    }
  }

  private fun clearBackdrop() {
    captureGeneration++
    backdropView.setImageDrawable(null)
    backdropBitmap?.recycle()
    backdropBitmap = null
  }
}
