package ai.gxlself.remotescreen.rsnative.session.zoom

import android.annotation.SuppressLint
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.drawable.Drawable
import android.os.Build
import android.widget.Magnifier
import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.SessionViewport
import ai.gxlself.remotescreen.logic.zoom.MagnifierPlacement
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import kotlin.math.roundToInt

/** API-29 magnifier overlay used by direct touch long-presses. */
class SessionMagnifier(
  private val view: RemoteSessionView,
  private val viewportProvider: () -> SessionViewport,
) {
  private val density = view.resources.displayMetrics.density
    .takeIf { it.isFinite() && it > 0f } ?: 1f
  @Suppress("DEPRECATION")
  private val scaledDensity = view.resources.displayMetrics.scaledDensity
    .takeIf { it.isFinite() && it > 0f } ?: density
  private var magnifier: Magnifier? = createMagnifier()
  private var visible = false
  private var cursor = GPoint(0.5f, 0.5f)
  private var finger = GPoint(0f, 0f)
  private var center = GPoint.ZERO

  fun setCursor(point: GPoint, fingerDp: GPoint? = null) {
    cursor = GPoint(point.x.coerceIn(0f, 1f), point.y.coerceIn(0f, 1f))
    if (fingerDp != null) finger = fingerDp
    if (visible) update()
  }

  fun showAt(fingerDp: GPoint) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
    finger = fingerDp
    visible = true
    update()
  }

  fun setVisible(on: Boolean) {
    if (on) {
      showAt(currentFinger())
    } else {
      visible = false
      dismissMagnifier()
      view.invalidateOverlay()
    }
  }

  fun hide() = setVisible(false)

  fun onDraw(canvas: Canvas) {
    if (!visible || Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
      color = Color.WHITE
      textSize = 12f * scaledDensity
      setShadowLayer(3f * density, 0f, 1f * density, Color.BLACK)
      textAlign = Paint.Align.CENTER
    }
    canvas.drawText(
      "松手单击",
      center.x * density,
      (center.y + MAGNIFIER_DIAMETER_DP / 2f + LABEL_GAP_DP) * density,
      paint,
    )
  }

  fun close() {
    visible = false
    dismissMagnifier()
    magnifier = null
  }

  @SuppressLint("NewApi")
  private fun createMagnifier(): Magnifier? {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
    val diameter = (MAGNIFIER_DIAMETER_DP * density).roundToInt()
    return Magnifier.Builder(view.surfaceView)
      .setSize(diameter, diameter)
      .setCornerRadius(diameter / 2f)
      .setInitialZoom(2f)
      .setOverlay(BorderDrawable(diameter / 2f, 2f * density))
      .build()
  }

  @SuppressLint("NewApi")
  private fun update() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || !visible) return
    val current = magnifier ?: return
    val viewport = viewportProvider()
    val point = viewport.viewPointFromNormalized(cursor)
    val surfaceLeftDp = view.surfaceView.left / density
    val surfaceTopDp = view.surfaceView.top / density
    val sourceX = (point.x - surfaceLeftDp).coerceIn(0f, view.surfaceView.width / density) * density
    val sourceY = (point.y - surfaceTopDp).coerceIn(0f, view.surfaceView.height / density) * density
    center = MagnifierPlacement.center(
      finger,
      view.height / density,
      diameterDp = MAGNIFIER_DIAMETER_DP,
      offsetDp = MAGNIFIER_OFFSET_DP,
    )
    val magnifierX = (center.x - surfaceLeftDp) * density
    val magnifierY = (center.y - surfaceTopDp) * density
    current.show(sourceX, sourceY, magnifierX, magnifierY)
    view.invalidateOverlay()
  }

  private fun currentFinger(): GPoint {
    val point = viewportProvider().viewPointFromNormalized(cursor)
    return point.takeIf { it.x.isFinite() && it.y.isFinite() } ?: GPoint.ZERO
  }

  @SuppressLint("NewApi")
  private fun dismissMagnifier() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) magnifier?.dismiss()
  }

  private class BorderDrawable(
    private val radius: Float,
    strokeWidth: Float,
  ) : Drawable() {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
      color = Color.WHITE
      style = Paint.Style.STROKE
      this.strokeWidth = strokeWidth
    }

    override fun draw(canvas: Canvas) {
      val bounds = bounds
      val inset = paint.strokeWidth / 2f
      canvas.drawCircle(bounds.exactCenterX(), bounds.exactCenterY(), radius - inset, paint)
    }

    override fun setAlpha(alpha: Int) {
      paint.alpha = alpha
    }

    override fun setColorFilter(colorFilter: android.graphics.ColorFilter?) {
      paint.colorFilter = colorFilter
    }

    @Deprecated("Deprecated in Java")
    override fun getOpacity(): Int = android.graphics.PixelFormat.TRANSLUCENT

    override fun getIntrinsicWidth(): Int = (radius * 2f).roundToInt()

    override fun getIntrinsicHeight(): Int = (radius * 2f).roundToInt()

    override fun onBoundsChange(bounds: Rect) = Unit
  }

  companion object {
    private const val MAGNIFIER_DIAMETER_DP = 120f
    private const val MAGNIFIER_OFFSET_DP = 80f
    private const val LABEL_GAP_DP = 20f
  }
}
