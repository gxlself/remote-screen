package ai.gxlself.remotescreen.rsnative.localauth

import android.app.Activity
import android.app.Application
import android.app.KeyguardManager
import android.annotation.SuppressLint
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import ai.gxlself.remotescreen.logic.localauth.LocalAuthBiometry
import ai.gxlself.remotescreen.logic.localauth.LocalAuthErrorMap
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.lang.ref.WeakReference

/** Application-scoped service. Mutable state and prompt/window operations stay on the main thread. */
class LocalAuthService private constructor(private val app: Application) {
  data class Capabilities(val biometrics: Boolean, val passcode: Boolean)

  private class Request(val completions: MutableList<(String) -> Unit>) {
    var capabilities = Capabilities(false, false)
    var biometry = "none"
    var prompt: BiometricPrompt? = null
    var host: WeakReference<FragmentActivity>? = null
    var recreating: ComponentName? = null
  }

  private val mainHandler = Handler(Looper.getMainLooper())
  private var request: Request? = null
  private var privacyCoverEnabled = false

  private val activityCallbacks = object : Application.ActivityLifecycleCallbacks {
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
      applyPrivacyCover(activity)
      val active = request ?: return
      if (activity is FragmentActivity && active.recreating == activity.componentName) {
        // BiometricPrompt retains authentication across configuration changes.
        // Reconnect the callback to its retained fragment without starting another prompt.
        try {
          attachPrompt(activity, active)
        } catch (error: RuntimeException) {
          finish(active, "failed", INTERNAL_ERROR, "reattach_${error.javaClass.simpleName}")
        }
      }
    }

    override fun onActivityResumed(activity: Activity) {
      applyPrivacyCover(activity)
      val active = request ?: return
      if (activity is FragmentActivity && active.host?.get() == null && !activity.isFinishing && !activity.isDestroyed) {
        // During a configuration change the new Activity may be resumed before
        // the old one receives onActivityDestroyed. Rebind to the retained
        // BiometricPrompt fragment at the first safe lifecycle point.
        try {
          attachPrompt(activity, active)
        } catch (error: RuntimeException) {
          finish(active, "failed", INTERNAL_ERROR, "reattach_${error.javaClass.simpleName}")
        }
      }
    }

    override fun onActivityDestroyed(activity: Activity) {
      val active = request ?: return
      if (active.host?.get() !== activity) return
      if (activity.isChangingConfigurations) {
        active.recreating = activity.componentName
        active.host = null
        active.prompt = null
      } else {
        active.prompt?.cancelAuthentication()
        finish(active, "cancelled", BiometricPrompt.ERROR_CANCELED)
      }
    }

    override fun onActivityStarted(activity: Activity) {}
    override fun onActivityPaused(activity: Activity) {}
    override fun onActivityStopped(activity: Activity) {}
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
  }

  init {
    mainHandler.post { app.registerActivityLifecycleCallbacks(activityCallbacks) }
  }

  // These read-only platform capability queries are also used by synchronous JS methods.
  fun canAuthenticate(): Capabilities = Capabilities(
    biometrics = BiometricManager.from(app).canAuthenticate(BIOMETRIC_WEAK) ==
      BiometricManager.BIOMETRIC_SUCCESS,
    passcode = (app.getSystemService(Context.KEYGUARD_SERVICE) as? KeyguardManager)?.isDeviceSecure == true,
  )

  fun getBiometryType(): String = biometryName(canAuthenticate().biometrics)

  fun authenticate(
    reason: String,
    @Suppress("UNUSED_PARAMETER") fallbackTitle: String,
    completion: (String) -> Unit,
  ) {
    mainHandler.post {
      val existing = request
      if (existing != null) {
        existing.completions.add(completion)
        return@post
      }

      val active = Request(mutableListOf(completion))
      request = active
      try {
        active.capabilities = canAuthenticate()
        active.biometry = biometryName(active.capabilities.biometrics)
        if (!active.capabilities.biometrics && !active.capabilities.passcode) {
          finish(active, "unavailable", BiometricPrompt.ERROR_NO_DEVICE_CREDENTIAL)
          return@post
        }

        val activity = RsActivityHooks.current
        if (activity !is FragmentActivity) {
          val cause = if (activity == null) "no_activity" else "activity_not_fragment_activity"
          finish(active, "unavailable", INTERNAL_ERROR, cause)
          return@post
        }
        if (activity.isFinishing || activity.isDestroyed) {
          finish(active, "unavailable", INTERNAL_ERROR, "activity_finishing_or_destroyed")
          return@post
        }

        // DEVICE_CREDENTIAL supplies the system PIN/pattern/password entry point.
        // A custom negative button (and fallbackTitle) cannot be used with it.
        val info = BiometricPrompt.PromptInfo.Builder()
          .setTitle(reason)
          .setAllowedAuthenticators(BIOMETRIC_WEAK or DEVICE_CREDENTIAL)
          .setConfirmationRequired(false)
          .build()
        attachPrompt(activity, active).authenticate(info)
      } catch (error: RuntimeException) {
        // Promise callers always receive a result, including prompt setup failures.
        finish(active, "failed", INTERNAL_ERROR, "authenticate_${error.javaClass.simpleName}")
      }
    }
  }

  fun setPrivacyCover(enabled: Boolean) {
    mainHandler.post {
      privacyCoverEnabled = enabled
      RsActivityHooks.current?.let(::applyPrivacyCover)
    }
  }

  fun onHostResume() {
    // MainActivity sets RsActivityHooks.current after super.onResume(), which
    // dispatches this React lifecycle event. Post to observe the updated hook.
    mainHandler.post { RsActivityHooks.current?.let(::applyPrivacyCover) }
  }

  @SuppressLint("InlinedApi")
  private fun biometryName(enrolled: Boolean): String {
    val pm = app.packageManager
    return LocalAuthBiometry.name(
      enrolled,
      pm.hasSystemFeature(PackageManager.FEATURE_FINGERPRINT),
      pm.hasSystemFeature(PackageManager.FEATURE_FACE),
      pm.hasSystemFeature(PackageManager.FEATURE_IRIS),
    )
  }

  private fun attachPrompt(activity: FragmentActivity, active: Request): BiometricPrompt {
    val prompt = BiometricPrompt(
      activity,
      ContextCompat.getMainExecutor(app),
      object : BiometricPrompt.AuthenticationCallback() {
        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
          finish(active, "ok", 0)
        }

        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
          val result = LocalAuthErrorMap.result(
            errorCode,
            active.capabilities.biometrics,
            active.capabilities.passcode,
          )
          finish(active, result, errorCode)
        }

        override fun onAuthenticationFailed() {
          // An unrecognized biometric is non-terminal; the system can retry or offer the PIN.
        }
      },
    )
    active.prompt = prompt
    active.host = WeakReference(activity)
    active.recreating = null
    return prompt
  }

  private fun applyPrivacyCover(activity: Activity) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      activity.setRecentsScreenshotEnabled(!privacyCoverEnabled)
    } else if (privacyCoverEnabled) {
      // Before Android 13 this also disables in-app screenshots and screen recording.
      activity.window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
    } else {
      activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }
  }

  private fun finish(active: Request, result: String, code: Int, cause: String? = null) {
    // Ignore late callbacks from a completed prompt, including cancellation after Activity destruction.
    if (request !== active) return
    val fields = linkedMapOf<String, Any?>(
      "result" to result,
      "code" to code,
      "biometry" to active.biometry,
    )
    if (cause != null) fields["reason"] = cause
    RsLog.marker("LOCALAUTH", fields)

    val pending = active.completions.toList()
    active.completions.clear()
    active.prompt = null
    active.host = null
    request = null
    pending.forEach { completion ->
      try {
        completion(result)
      } catch (error: RuntimeException) {
        // One invalidated React caller must not prevent the other waiters from resolving.
        RsLog.marker("LOCALAUTH", mapOf("callbackError" to error.javaClass.simpleName))
      }
    }
  }

  companion object {
    private const val INTERNAL_ERROR = -1
    private var instance: LocalAuthService? = null

    @Synchronized
    fun get(context: Context): LocalAuthService = instance ?: LocalAuthService(
      context.applicationContext as Application,
    ).also { instance = it }
  }
}
