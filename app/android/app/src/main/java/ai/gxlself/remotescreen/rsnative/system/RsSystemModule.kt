package ai.gxlself.remotescreen.rsnative.system

import android.Manifest
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.hardware.display.DisplayManager
import android.os.Build
import android.view.Display
import androidx.annotation.RequiresApi
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.UiThreadUtil
import com.facebook.react.modules.core.PermissionAwareActivity
import com.facebook.react.modules.core.PermissionListener
import ai.gxlself.remotescreen.logic.system.HapticKind
import ai.gxlself.remotescreen.logic.system.HapticMap
import ai.gxlself.remotescreen.logic.system.MaxFps
import ai.gxlself.remotescreen.logic.system.NotificationStatus
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.specs.NativeRsSystemSpec

class RsSystemModule(
  reactContext: ReactApplicationContext,
) : NativeRsSystemSpec(reactContext) {
  private val notificationRequestLock = Any()
  private var notificationRequestInFlight = false

  override fun getName(): String = NAME

  override fun getTypedExportedConstants(): Map<String, Any> {
    val packageInfo = packageInfoOrNull()
    val constants = mapOf(
      "appVersion" to (packageInfo?.versionName ?: ""),
      "buildNumber" to (packageInfo?.let(::buildNumber) ?: "0"),
      "bundleId" to reactApplicationContext.packageName,
      "modelId" to "${Build.MANUFACTURER} ${Build.MODEL}".trim(),
      "maxFps" to displayMaxFps(),
    )
    mark("getTypedExportedConstants", "ok")
    return constants
  }

  override fun copyText(text: String) {
    try {
      val clipboard = reactApplicationContext.getSystemService(ClipboardManager::class.java)
      if (clipboard == null) {
        mark("copyText", "error")
        return
      }
      clipboard.setPrimaryClip(ClipData.newPlainText("Remote Screen", text))
      mark("copyText", "ok")
    } catch (_: Exception) {
      mark("copyText", "error")
    }
  }

  override fun hasText(): Boolean {
    return try {
      val clipboard = reactApplicationContext.getSystemService(ClipboardManager::class.java)
      val hasText = clipboard?.hasPrimaryClip() == true &&
        clipboard.primaryClipDescription?.hasMimeType("text/*") == true
      mark("hasText", hasText)
      hasText
    } catch (_: Exception) {
      mark("hasText", "error")
      false
    }
  }

  // ClipboardManager keeps no change count.
  override fun clipboardChangeCount(): Double = -1.0

  override fun readText(promise: Promise) {
    try {
      val clipboard = reactApplicationContext.getSystemService(ClipboardManager::class.java)
      val value = clipboard?.primaryClip
        ?.takeIf { it.itemCount > 0 }
        ?.getItemAt(0)
        ?.coerceToText(reactApplicationContext)
        ?.toString()
      promise.resolve(value)
      mark("readText", if (value == null) "null" else "text")
    } catch (_: Exception) {
      promise.resolve(null)
      mark("readText", "error")
    }
  }

  override fun haptic(kind: String) {
    val parsed = HapticKind.parse(kind)
    if (parsed == null) {
      mark("haptic", "ignored")
      return
    }

    UiThreadUtil.runOnUiThread {
      val view = reactApplicationContext.currentActivity?.window?.decorView
      if (view == null) {
        mark("haptic", "ignored")
        return@runOnUiThread
      }
      val performed = view.performHapticFeedback(
        HapticMap.constantFor(parsed, Build.VERSION.SDK_INT),
      )
      mark("haptic", if (performed) "performed" else "ignored")
    }
  }

  override fun notificationStatus(promise: Promise) {
    val status = notificationStatusValue()
    promise.resolve(status)
    mark("notificationStatus", status)
  }

  override fun requestNotifications(promise: Promise) {
    val sdk = Build.VERSION.SDK_INT
    val permissionGranted = postNotificationsPermissionGranted()
    if (sdk < ANDROID_13 || permissionGranted) {
      val result = notificationResult(notificationStatusValue())
      promise.resolve(result)
      mark("requestNotifications", result)
      return
    }

    var alreadyInFlight = false
    synchronized(notificationRequestLock) {
      if (notificationRequestInFlight) {
        alreadyInFlight = true
      } else {
        notificationRequestInFlight = true
        requestedNotificationsPreferences()
          .edit()
          .putBoolean(NOTIFICATIONS_REQUESTED_KEY, true)
          .apply()
      }
    }
    if (alreadyInFlight) {
      val result = notificationResult(notificationStatusValue())
      promise.resolve(result)
      mark("requestNotifications", result)
      return
    }

    val activity = reactApplicationContext.currentActivity
    if (activity !is PermissionAwareActivity) {
      finishNotificationRequest()
      promise.resolve("denied")
      mark("requestNotifications", "denied")
      return
    }

    if (Build.VERSION.SDK_INT < ANDROID_13) {
      finishNotificationRequest()
      promise.resolve("denied")
      mark("requestNotifications", "denied")
      return
    }
    requestNotificationsOnAndroid13(activity, promise)
  }

  override fun requestLocalNetwork(timeoutMs: Double, promise: Promise) {
    promise.resolve("granted")
    mark("requestLocalNetwork", "granted")
  }

  private fun packageInfoOrNull(): PackageInfo? = try {
    reactApplicationContext.packageManager.getPackageInfo(
      reactApplicationContext.packageName,
      0,
    )
  } catch (_: PackageManager.NameNotFoundException) {
    null
  }

  @Suppress("DEPRECATION")
  private fun buildNumber(packageInfo: PackageInfo): String =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
      packageInfo.longVersionCode.toString()
    } else {
      packageInfo.versionCode.toString()
    }

  private fun displayMaxFps(): Int {
    val displayManager = reactApplicationContext.getSystemService(DisplayManager::class.java)
    val display = displayManager?.getDisplay(Display.DEFAULT_DISPLAY)
    return MaxFps.maxFps(display?.supportedModes?.map { it.refreshRate } ?: emptyList())
  }

  private fun notificationStatusValue(): String = NotificationStatus.status(
    sdk = Build.VERSION.SDK_INT,
    permissionGranted = postNotificationsPermissionGranted(),
    notificationsEnabled = notificationsEnabled(),
    requestedBefore = requestedNotificationsPreferences()
      .getBoolean(NOTIFICATIONS_REQUESTED_KEY, false),
  )

  private fun postNotificationsPermissionGranted(): Boolean =
    if (Build.VERSION.SDK_INT < ANDROID_13) {
      true
    } else {
      reactApplicationContext.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED
    }

  private fun notificationsEnabled(): Boolean {
    // On Android 13+, an app before its first permission request can report
    // notifications as disabled even though the user has not disabled them
    // in Settings. Let NotificationStatus use requestedBefore for that state.
    if (Build.VERSION.SDK_INT >= ANDROID_13 &&
      !postNotificationsPermissionGranted() &&
      !requestedNotificationsPreferences().getBoolean(NOTIFICATIONS_REQUESTED_KEY, false)
    ) {
      return true
    }
    return reactApplicationContext
      .getSystemService(NotificationManager::class.java)
      ?.areNotificationsEnabled() == true
  }

  private fun requestedNotificationsPreferences() =
    reactApplicationContext.getSharedPreferences(SYSTEM_PREFERENCES, Context.MODE_PRIVATE)

  private fun notificationResult(status: String): String =
    if (status == "granted") "granted" else "denied"

  @RequiresApi(Build.VERSION_CODES.TIRAMISU)
  private fun requestNotificationsOnAndroid13(
    activity: PermissionAwareActivity,
    promise: Promise,
  ) {
    try {
      activity.requestPermissions(
        arrayOf(Manifest.permission.POST_NOTIFICATIONS),
        NOTIFICATION_REQUEST_CODE,
        object : PermissionListener {
          override fun onRequestPermissionsResult(
            requestCode: Int,
            permissions: Array<String>,
            grantResults: IntArray,
          ): Boolean {
            if (requestCode != NOTIFICATION_REQUEST_CODE) {
              return false
            }
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            val result = if (granted && notificationsEnabled()) "granted" else "denied"
            finishNotificationRequest()
            promise.resolve(result)
            mark("requestNotifications", result)
            return true
          }
        },
      )
    } catch (_: Exception) {
      finishNotificationRequest()
      promise.resolve("denied")
      mark("requestNotifications", "denied")
    }
  }

  private fun finishNotificationRequest() {
    synchronized(notificationRequestLock) {
      notificationRequestInFlight = false
    }
  }

  private fun mark(method: String, result: Any?) {
    RsLog.marker("SYSTEM", mapOf(method to result))
  }

  companion object {
    const val NAME = "RsSystem"

    private const val ANDROID_13 = 33
    private const val NOTIFICATION_REQUEST_CODE = 0x5253
    private const val SYSTEM_PREFERENCES = "rs.system"
    private const val NOTIFICATIONS_REQUESTED_KEY = "notifications.requested.v1"
  }
}
