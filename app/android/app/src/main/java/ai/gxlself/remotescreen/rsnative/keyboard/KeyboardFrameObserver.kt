package ai.gxlself.remotescreen.rsnative.keyboard

import android.content.res.Configuration
import android.view.View
import android.view.ViewTreeObserver
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/** Reports the visible IME frame and whether a physical keyboard is attached. */
class KeyboardFrameObserver(
  private val view: View,
  private val onChange: (heightDp: Float, visible: Boolean, hardware: Boolean) -> Unit,
) {
  private var started = false
  private var last: Frame? = null

  private val listener = ViewTreeObserver.OnGlobalLayoutListener { update() }

  fun start() {
    if (started) return
    started = true
    view.viewTreeObserver.addOnGlobalLayoutListener(listener)
    ViewCompat.requestApplyInsets(view)
    view.post { if (started) update() }
  }

  fun stop() {
    if (!started) return
    if (view.viewTreeObserver.isAlive) {
      view.viewTreeObserver.removeOnGlobalLayoutListener(listener)
    }
    started = false
    last = null
  }

  private fun update() {
    if (!started) return
    val insets = ViewCompat.getRootWindowInsets(view) ?: return
    val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
    val navigation = insets.getInsets(WindowInsetsCompat.Type.navigationBars())
    // IME insets include the bottom navigation area on some Android versions.
    val heightPx = (ime.bottom - navigation.bottom).coerceAtLeast(0)
    val density = view.resources.displayMetrics.density
    val heightDp = heightPx / density
    val visible = insets.isVisible(WindowInsetsCompat.Type.ime())
    val configuration = view.resources.configuration
    val hardware =
      configuration.keyboard == Configuration.KEYBOARD_QWERTY &&
        configuration.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO
    val next = Frame(heightDp, visible, hardware)
    if (next == last) return
    last = next
    onChange(next.heightDp, next.visible, next.hardware)
  }

  private data class Frame(val heightDp: Float, val visible: Boolean, val hardware: Boolean)
}
