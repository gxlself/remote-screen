package ai.gxlself.remotescreen.rsnative.androidsession.pip

import android.app.Activity
import android.app.Application
import android.app.PictureInPictureParams
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Rational
import ai.gxlself.remotescreen.logic.pip.PipAspect
import ai.gxlself.remotescreen.logic.pip.PipPolicy
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.lang.ref.WeakReference

/** Bridges the session PiP state to the currently visible Android activity. */
object PipBridge {
  private val lock = Any()
  private val mainHandler = Handler(Looper.getMainLooper())

  @Volatile private var application: Application? = null
  @Volatile private var listenerInstalled = false
  @Volatile private var supported: Boolean? = null
  @Volatile private var unsupportedLogged = false

  private var eligible = false
  private var aspectWidth = 16
  private var aspectHeight = 9
  private var lastActivity: WeakReference<Activity>? = null
  private var lastInPip = false

  private val activityListener = object : RsActivityHooks.Listener {
    override fun onUserLeaveHint(activity: Activity) {
      handleUserLeaveHint(activity)
    }

    override fun onPictureInPictureModeChanged(activity: Activity, inPip: Boolean) {
      handlePipChanged(activity, inPip)
    }
  }

  @Volatile
  var onPipChange: ((Boolean) -> Unit)? = null

  /** Supplies the application context when the native module is initialized. */
  fun install(app: Application) {
    ensureInitialized(app = app)
  }

  fun setEligible(eligible: Boolean, w: Int, h: Int) {
    val isSupported = ensureInitialized()
    synchronized(lock) {
      this.eligible = eligible
      aspectWidth = w
      aspectHeight = h
    }
    if (isSupported != true) {
      return
    }
    runOnMain { applyParams() }
  }

  fun isInPip(): Boolean {
    if (ensureInitialized() != true) {
      return false
    }
    val activity = synchronized(lock) { lastActivity?.get() }
    return activity?.isInPictureInPictureMode ?: synchronized(lock) { lastInPip }
  }

  private fun ensureInitialized(
    app: Application? = null,
    activity: Activity? = RsActivityHooks.current,
  ): Boolean? {
    synchronized(lock) {
      if (!listenerInstalled) {
        RsActivityHooks.add(activityListener)
        listenerInstalled = true
      }
      if (app != null) {
        application = app
      } else if (application == null) {
        application = activity?.application
      }
      if (activity != null) {
        lastActivity = WeakReference(activity)
      }
      if (supported == null) {
        val context = application ?: activity?.application
        if (context != null) {
          supported = context.packageManager.hasSystemFeature(
            PackageManager.FEATURE_PICTURE_IN_PICTURE,
          )
          if (supported == false && !unsupportedLogged) {
            unsupportedLogged = true
            RsLog.marker("PIP", mapOf("supported" to false))
          }
        }
      }
      return supported
    }
  }

  private fun handleUserLeaveHint(activity: Activity) {
    if (ensureInitialized(activity = activity) != true) {
      return
    }
    synchronized(lock) {
      lastActivity = WeakReference(activity)
    }
    runOnMain {
      val state = synchronized(lock) {
        PiPState(eligible, aspectWidth, aspectHeight)
      }
      if (!PipPolicy.enterOnLeaveHint(Build.VERSION.SDK_INT, state.eligible, supported == true)) {
        return@runOnMain
      }
      try {
        activity.enterPictureInPictureMode(buildParams(state))
      } catch (_: IllegalStateException) {
        RsLog.marker("PIP", mapOf("enter" to false, "error" to "IllegalStateException"))
      }
    }
  }

  private fun handlePipChanged(activity: Activity, inPip: Boolean) {
    if (ensureInitialized(activity = activity) != true) {
      return
    }
    synchronized(lock) {
      lastActivity = WeakReference(activity)
      lastInPip = inPip
    }
    RsLog.marker("PIP", mapOf("inPip" to inPip))
    onPipChange?.invoke(inPip)
  }

  private fun applyParams() {
    val activity = RsActivityHooks.current ?: synchronized(lock) { lastActivity?.get() }
    if (activity == null || supported != true) {
      return
    }
    val state = synchronized(lock) {
      PiPState(eligible, aspectWidth, aspectHeight)
    }
    activity.setPictureInPictureParams(buildParams(state))
  }

  private fun buildParams(state: PiPState): PictureInPictureParams {
    val (width, height) = PipAspect.clamp(state.width, state.height)
    return PictureInPictureParams.Builder()
      .setAspectRatio(Rational(width, height))
      .setActions(emptyList())
      .apply {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
          setAutoEnterEnabled(state.eligible)
          setSeamlessResizeEnabled(true)
        }
      }
      .build()
  }

  private fun runOnMain(action: () -> Unit) {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      action()
    } else {
      mainHandler.post(action)
    }
  }

  private data class PiPState(
    val eligible: Boolean,
    val width: Int,
    val height: Int,
  )
}
