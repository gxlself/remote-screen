package ai.gxlself.remotescreen.rsnative.lifecycle

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.SharedPreferences
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import ai.gxlself.remotescreen.logic.lifecycle.BatteryMapper
import ai.gxlself.remotescreen.logic.lifecycle.BatteryKind
import ai.gxlself.remotescreen.logic.lifecycle.ExitMarker
import ai.gxlself.remotescreen.logic.lifecycle.KeyValueBool
import ai.gxlself.remotescreen.logic.lifecycle.PowerPolicy
import ai.gxlself.remotescreen.logic.lifecycle.PowerState
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.util.concurrent.CopyOnWriteArraySet

data class BackgroundTaskEvent(
  val event: String,
  val reason: String,
  val heldMs: Double,
)

/** Main-thread owner of process lifecycle, battery state, and JS event callbacks. */
object LifecycleMonitor {
  interface Emitter {
    fun onPowerChange(state: PowerState)

    fun onBackgroundTask(event: BackgroundTaskEvent)
  }

  private const val PREFERENCES_FILE = "rs.lifecycle"

  private val mainHandler: Handler by lazy { Handler(Looper.getMainLooper()) }
  private val emitters = CopyOnWriteArraySet<Emitter>()
  private val lifecycleObserver = object : DefaultLifecycleObserver {
    override fun onStop(owner: LifecycleOwner) {
      enterBackground()
    }

    override fun onStart(owner: LifecycleOwner) {
      enterForeground()
    }
  }
  private val activityCallbacks = object : Application.ActivityLifecycleCallbacks {
    override fun onActivityResumed(activity: Activity) {
      applyIdleTimer(activity)
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: android.os.Bundle?) = Unit

    override fun onActivityStarted(activity: Activity) = Unit

    override fun onActivityPaused(activity: Activity) = Unit

    override fun onActivityStopped(activity: Activity) = Unit

    override fun onActivitySaveInstanceState(activity: Activity, outState: android.os.Bundle) = Unit

    override fun onActivityDestroyed(activity: Activity) = Unit
  }
  private val powerReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
      if (intent.action == Intent.ACTION_BATTERY_CHANGED ||
        intent.action == PowerManager.ACTION_POWER_SAVE_MODE_CHANGED
      ) {
        updatePower(
          readBatteryIntent(if (intent.action == Intent.ACTION_BATTERY_CHANGED) intent else null),
        )
      }
    }
  }

  @Volatile private var started = false
  @Volatile private var app: Application? = null
  @Volatile private var previousRunEndedInBackground = false
  @Volatile private var currentPower = PowerPolicy.state(false, -1.0, BatteryKind.UNKNOWN)
  @Volatile private var idleTimerDisabled = false
  @Volatile private var keepAlive = false

  private var inBackground = false
  private var backgroundStartedAtMs: Long? = null
  private var powerManager: PowerManager? = null
  private var batteryLevel = -1.0
  private var batteryKind = BatteryKind.UNKNOWN

  fun start(application: Application) {
    runOnMain {
      startOnMain(application)
    }
  }

  fun previousRunEndedInBackground(): Boolean = previousRunEndedInBackground

  fun currentPowerState(): PowerState = currentPower

  fun registerEmitter(emitter: Emitter) {
    runOnMain {
      startIfNeeded()
      emitters.add(emitter)
    }
  }

  fun unregisterEmitter(emitter: Emitter) {
    runOnMain {
      emitters.remove(emitter)
    }
  }

  fun setBackgroundKeepAlive(enabled: Boolean) {
    runOnMain {
      startIfNeeded()
      keepAlive = enabled
      if (!enabled) {
        finishBackgroundTask("disabled")
      } else if (inBackground) {
        beginBackgroundTaskIfNeeded()
      }
    }
  }

  fun setIdleTimerDisabled(disabled: Boolean) {
    runOnMain {
      idleTimerDisabled = disabled
      RsActivityHooks.current?.let(::applyIdleTimer)
      RsLog.marker("LIFECYCLE", mapOf("idleTimerDisabled" to disabled))
    }
  }

  private fun startIfNeeded() {
    val application = app
    if (!started && application != null) startOnMain(application)
  }

  private fun startOnMain(application: Application) {
    check(Looper.myLooper() == Looper.getMainLooper()) { "LifecycleMonitor must start on the main thread" }
    if (started) return
    started = true
    app = application
    val preferences = application.getSharedPreferences(PREFERENCES_FILE, Context.MODE_PRIVATE)
    previousRunEndedInBackground = ExitMarker(SharedPreferencesBool(preferences)).start()

    powerManager = application.getSystemService(PowerManager::class.java)
    val batteryFilter = IntentFilter().apply {
      addAction(Intent.ACTION_BATTERY_CHANGED)
      addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED)
    }
    val stickyBattery = registerPowerReceiver(application, batteryFilter)
    currentPower = readBatteryIntent(stickyBattery)

    val processLifecycle = ProcessLifecycleOwner.get().lifecycle
    processLifecycle.addObserver(lifecycleObserver)
    inBackground = !processLifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    if (inBackground) {
      marker()?.markBackground()
    }
    application.registerActivityLifecycleCallbacks(activityCallbacks)
    RsLog.marker(
      "LIFECYCLE",
      mapOf("event" to "start", "previousRunEndedInBackground" to previousRunEndedInBackground),
    )
  }

  @Suppress("DEPRECATION", "UnspecifiedRegisterReceiverFlag")
  private fun registerPowerReceiver(application: Application, filter: IntentFilter): Intent? {
    return if (Build.VERSION.SDK_INT >= 33) {
      application.registerReceiver(powerReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
    } else {
      application.registerReceiver(powerReceiver, filter)
    }
  }

  private fun enterBackground() {
    check(Looper.myLooper() == Looper.getMainLooper())
    inBackground = true
    marker()?.markBackground()
    if (keepAlive) beginBackgroundTaskIfNeeded()
  }

  private fun enterForeground() {
    check(Looper.myLooper() == Looper.getMainLooper())
    inBackground = false
    marker()?.markForeground()
    finishBackgroundTask("foreground")
  }

  private fun marker(): ExitMarker? {
    val application = app ?: return null
    return ExitMarker(
      SharedPreferencesBool(application.getSharedPreferences(PREFERENCES_FILE, Context.MODE_PRIVATE)),
    )
  }

  private fun beginBackgroundTaskIfNeeded() {
    if (backgroundStartedAtMs != null) return
    backgroundStartedAtMs = SystemClock.elapsedRealtime()
    logBackground("bgBegin", null, 0.0)
    dispatchBackground(BackgroundTaskEvent("begin", "", 0.0))
  }

  private fun finishBackgroundTask(reason: String) {
    val startedAt = backgroundStartedAtMs ?: return
    backgroundStartedAtMs = null
    val heldMs = (SystemClock.elapsedRealtime() - startedAt).coerceAtLeast(0L).toDouble()
    logBackground("bgEnd", reason, heldMs)
    dispatchBackground(BackgroundTaskEvent("end", reason, heldMs))
  }

  private fun logBackground(event: String, reason: String?, heldMs: Double) {
    val fields = linkedMapOf<String, Any?>("event" to event)
    if (reason != null) fields["reason"] = reason
    if (event == "bgEnd") fields["heldMs"] = heldMs
    RsLog.marker("LIFECYCLE", fields)
  }

  private fun readBatteryIntent(intent: Intent?): PowerState {
    if (intent != null) {
      batteryLevel = BatteryMapper.level(
        intent.getIntExtra(android.os.BatteryManager.EXTRA_LEVEL, -1),
        intent.getIntExtra(android.os.BatteryManager.EXTRA_SCALE, -1),
      )
      batteryKind = BatteryMapper.kind(
        intent.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, Int.MIN_VALUE),
        intent.getIntExtra(android.os.BatteryManager.EXTRA_PLUGGED, 0),
      )
    }
    return PowerPolicy.state(powerManager?.isPowerSaveMode == true, batteryLevel, batteryKind)
  }

  private fun updatePower(next: PowerState) {
    if (!PowerPolicy.changed(currentPower, next)) return
    currentPower = next
    RsLog.marker(
      "LIFECYCLE",
      mapOf(
        "event" to "power",
        "lowPower" to next.lowPower,
        "lowPowerMode" to next.lowPowerMode,
        "batteryLevel" to next.batteryLevel,
        "charging" to next.charging,
      ),
    )
    emitters.forEach { emitter ->
      try {
        emitter.onPowerChange(next)
      } catch (error: Throwable) {
        RsLog.marker("LIFECYCLE", mapOf("event" to "powerEmitterError", "message" to (error.message ?: error::class.java.simpleName)))
      }
    }
  }

  private fun dispatchBackground(event: BackgroundTaskEvent) {
    emitters.forEach { emitter ->
      try {
        emitter.onBackgroundTask(event)
      } catch (error: Throwable) {
        RsLog.marker("LIFECYCLE", mapOf("event" to "backgroundEmitterError", "message" to (error.message ?: error::class.java.simpleName)))
      }
    }
  }

  private fun applyIdleTimer(activity: Activity) {
    if (idleTimerDisabled) {
      activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    } else {
      activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }
  }

  private fun runOnMain(action: () -> Unit) {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      action()
    } else {
      mainHandler.post(action)
    }
  }

  private class SharedPreferencesBool(private val preferences: SharedPreferences) : KeyValueBool {
    override fun get(key: String): Boolean = preferences.getBoolean(key, false)

    override fun set(key: String, value: Boolean) {
      preferences.edit().putBoolean(key, value).commit()
    }
  }
}
