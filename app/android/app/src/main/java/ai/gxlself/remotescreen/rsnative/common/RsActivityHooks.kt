package ai.gxlself.remotescreen.rsnative.common

import android.app.Activity
import android.content.Intent
import java.lang.ref.WeakReference
import java.util.concurrent.CopyOnWriteArraySet

/** Activity callbacks shared by native feature packages. */
object RsActivityHooks {
  interface Listener {
    fun onUserLeaveHint(activity: Activity) {}

    fun onPictureInPictureModeChanged(activity: Activity, inPip: Boolean) {}

    fun onNewIntent(activity: Activity, intent: Intent) {}
  }

  private val listeners = CopyOnWriteArraySet<Listener>()

  @Volatile private var currentRef: WeakReference<Activity>? = null

  val current: Activity?
    get() = currentRef?.get()

  fun add(l: Listener) {
    listeners.add(l)
  }

  fun remove(l: Listener) {
    listeners.remove(l)
  }

  internal fun setCurrent(a: Activity?) {
    currentRef = a?.let(::WeakReference)
  }

  internal fun dispatchUserLeaveHint(a: Activity) {
    listeners.forEach { listener ->
      try {
        listener.onUserLeaveHint(a)
      } catch (error: Throwable) {
        logError(error)
      }
    }
  }

  internal fun dispatchPipChanged(a: Activity, inPip: Boolean) {
    listeners.forEach { listener ->
      try {
        listener.onPictureInPictureModeChanged(a, inPip)
      } catch (error: Throwable) {
        logError(error)
      }
    }
  }

  internal fun dispatchNewIntent(a: Activity, i: Intent) {
    listeners.forEach { listener ->
      try {
        listener.onNewIntent(a, i)
      } catch (error: Throwable) {
        logError(error)
      }
    }
  }

  private fun logError(error: Throwable) {
    RsLog.marker(
      "HOOK",
      mapOf("error" to (error.message ?: error::class.java.simpleName)),
    )
  }
}
