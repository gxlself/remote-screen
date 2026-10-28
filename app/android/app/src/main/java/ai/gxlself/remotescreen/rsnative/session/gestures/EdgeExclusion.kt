package ai.gxlself.remotescreen.rsnative.session.gestures

import android.graphics.Rect
import android.os.Build
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import kotlin.math.roundToInt

/** Applies the Android 10+ edge exclusion rectangles required by a session. */
object EdgeExclusion {
  private const val WIDTH_DP = 24f
  private const val HEIGHT_DP = 200f

  fun update(view: RemoteSessionView) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
    if (view.sessionLong == null || view.width <= 0 || view.height <= 0) {
      view.systemGestureExclusionRects = emptyList()
      return
    }
    val density = view.resources.displayMetrics.density.takeIf { it.isFinite() && it > 0f } ?: 1f
    val width = (WIDTH_DP * density).roundToInt().coerceAtLeast(1)
    val height = (HEIGHT_DP * density).roundToInt().coerceAtLeast(1).coerceAtMost(view.height)
    val top = ((view.height - height) / 2).coerceAtLeast(0)
    view.systemGestureExclusionRects = listOf(
      Rect(0, top, width.coerceAtMost(view.width), top + height),
      Rect((view.width - width).coerceAtLeast(0), top, view.width, top + height),
    )
  }

  fun clear(view: RemoteSessionView) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      view.systemGestureExclusionRects = emptyList()
    }
  }
}
