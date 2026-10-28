package ai.gxlself.remotescreen.rsnative.session.gestures

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.CloseOnce
import ai.gxlself.remotescreen.logic.gestures.HostCursorArbiter
import ai.gxlself.remotescreen.logic.gestures.CursorModel
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.rsnative.jni.SessionSink
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import java.util.LinkedHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.ThreadFactory
import kotlin.math.max
import kotlin.math.min

/** Owns the local cursor model and draws it on RemoteSessionView's overlay. */
class SessionCursorController(
  private val view: RemoteSessionView,
  private val viewportProvider: () -> SessionViewport,
  private val onCursorMoved: (GPoint) -> Unit = {},
) {
  private data class Shape(val bitmap: Bitmap, val hotX: Int, val hotY: Int, val scale: Float)

  private val density = view.resources.displayMetrics.density.takeIf { it.isFinite() && it > 0f } ?: 1f
  private val decodeExecutor: ExecutorService =
    Executors.newSingleThreadExecutor(CursorThreadFactory)
  private val shapes = object : LinkedHashMap<Long, Shape>(SHAPE_CACHE_SIZE, 0.75f, true) {
    override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long, Shape>?): Boolean =
      size > SHAPE_CACHE_SIZE
  }
  private val model = CursorModel()
  private val hostArbiter = HostCursorArbiter()
  private val arrowPath = Path().apply {
    moveTo(0.5f, 0.5f)
    lineTo(0.5f, 20.5f)
    lineTo(5.3f, 16f)
    lineTo(9f, 23.3f)
    lineTo(12f, 21.8f)
    lineTo(8.3f, 14.5f)
    lineTo(15.2f, 14.5f)
    close()
  }
  private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0xffffffff.toInt(); style = Paint.Style.FILL }
  private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
    color = 0xff000000.toInt()
    style = Paint.Style.STROKE
    strokeWidth = 1f
    strokeJoin = Paint.Join.ROUND
  }
  private var generation = 0L
  private var requestedShapeId: Long? = null
  private var selectedShape: Shape? = null
  private var hostVisible = true
  private var videoWidthPx = 0f
  private val closeOnce = CloseOnce {
    generation += 1
    decodeExecutor.shutdown()
  }

  val position: GPoint
    get() = model.position

  fun setVideoSize(width: Int, height: Int) {
    if (width > 0 && height > 0) {
      videoWidthPx = width.toFloat()
      view.invalidateOverlay()
    }
  }

  fun moveBy(viewDeltaDp: GPoint, gain: Float): GPoint {
    val viewport = viewportProvider()
    model.apply(viewDeltaDp, viewport, gain)
    hostArbiter.recordLocalMove(nowSeconds())
    onCursorMoved(model.position)
    view.invalidateOverlay()
    return model.position
  }

  fun moveTo(point: GPoint, markLocal: Boolean = true): GPoint {
    model.moveTo(point)
    if (markLocal) hostArbiter.recordLocalMove(nowSeconds())
    if (markLocal) onCursorMoved(model.position)
    view.invalidateOverlay()
    return model.position
  }

  fun center() {
    moveTo(GPoint(0.5f, 0.5f))
  }

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
  ) {
    if (closeOnce.isClosed) return
    when (kind) {
      SessionSink.EV_STREAM_CONFIG,
      SessionSink.EV_CURSOR_POS -> {
        hostVisible = visible
        if (kind == SessionSink.EV_STREAM_CONFIG || hostArbiter.shouldAcceptHost(nowSeconds())) {
          model.moveTo(GPoint(x, y))
          onCursorMoved(model.position)
        }
        view.invalidateOverlay()
      }
      SessionSink.EV_CURSOR_SHAPE -> decodeShape(shapeId, hotX, hotY, scale, png)
      SessionSink.EV_CURSOR_USE -> {
        requestedShapeId = shapeId
        selectedShape = shapes[shapeId]
        view.invalidateOverlay()
      }
      SessionSink.EV_CURSOR_HIDDEN -> {
        hostVisible = visible
        view.invalidateOverlay()
      }
    }
  }

  fun draw(canvas: Canvas) {
    val viewport = viewportProvider()
    val rect = viewport.contentRect
    if (!hostVisible || view.sessionLong == null || rect.w <= 0f || rect.h <= 0f) return
    val tip = viewport.viewPointFromNormalized(model.position)
    if (!tip.x.isFinite() || !tip.y.isFinite()) return

    val shape = selectedShape
    if (shape == null) {
      canvas.save()
      canvas.translate(tip.x * density, tip.y * density)
      canvas.scale(density, density)
      canvas.drawPath(arrowPath, fill)
      canvas.drawPath(arrowPath, stroke)
      canvas.restore()
      return
    }

    val sourceWidth = view.surfaceView.holder.surfaceFrame.width().toFloat().takeIf { it > 0f } ?: videoWidthPx
    val ratio = if (sourceWidth > 0f) {
      (rect.w / sourceWidth).takeIf { it.isFinite() && it > 0f } ?: 1f
    } else {
      1f / shape.scale
    }
    val widthDp = min(max(shape.bitmap.width * ratio, MIN_SHAPE_DP), MAX_SHAPE_DP)
    val heightDp = min(max(shape.bitmap.height * ratio, MIN_SHAPE_DP), MAX_SHAPE_DP)
    val hotspotX = shape.hotX.toFloat() * widthDp / shape.bitmap.width.coerceAtLeast(1)
    val hotspotY = shape.hotY.toFloat() * heightDp / shape.bitmap.height.coerceAtLeast(1)
    val destination = RectF(
      (tip.x - hotspotX) * density,
      (tip.y - hotspotY) * density,
      (tip.x - hotspotX + widthDp) * density,
      (tip.y - hotspotY + heightDp) * density,
    )
    canvas.drawBitmap(shape.bitmap, null, destination, null)
  }

  fun reset() {
    generation += 1
    requestedShapeId = null
    selectedShape = null
    hostVisible = true
    model.moveTo(GPoint(0.5f, 0.5f))
    shapes.clear()
    view.invalidateOverlay()
  }

  fun close() {
    closeOnce.close()
  }

  private fun decodeShape(shapeId: Long, hotX: Int, hotY: Int, scale: Float, png: ByteArray?) {
    if (closeOnce.isClosed || png == null || png.isEmpty()) return
    requestedShapeId = shapeId
    shapes[shapeId]?.let {
      selectedShape = it
      view.invalidateOverlay()
      return
    }
    val expectedGeneration = generation
    val bytes = png.copyOf()
    decodeExecutor.execute {
      val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
      if (bitmap == null) return@execute
      val safeHotX = hotX.coerceIn(0, bitmap.width)
      val safeHotY = hotY.coerceIn(0, bitmap.height)
      view.post {
        if (expectedGeneration != generation) {
          bitmap.recycle()
          return@post
        }
        val shape = Shape(
          bitmap = bitmap,
          hotX = safeHotX,
          hotY = safeHotY,
          scale = scale.takeIf { it.isFinite() && it > 0f } ?: 1f,
        )
        shapes[shapeId] = shape
        if (requestedShapeId == shapeId) selectedShape = shape
        view.invalidateOverlay()
      }
    }
  }

  private fun nowSeconds(): Double = System.nanoTime() / 1_000_000_000.0

  private object CursorThreadFactory : ThreadFactory {
    override fun newThread(runnable: Runnable): Thread =
      Thread(runnable, "rs-cursor-decode").apply { isDaemon = true }
  }

  companion object {
    private const val SHAPE_CACHE_SIZE = 64
    private const val MIN_SHAPE_DP = 12f
    private const val MAX_SHAPE_DP = 64f
  }
}
