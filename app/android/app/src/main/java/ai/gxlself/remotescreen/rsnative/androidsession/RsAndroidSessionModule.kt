package ai.gxlself.remotescreen.rsnative.androidsession

import android.content.Intent
import androidx.core.content.ContextCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.androidsession.pip.PipBridge
import ai.gxlself.remotescreen.rsnative.androidsession.tile.TileBridge
import ai.gxlself.remotescreen.specs.NativeRsAndroidSessionSpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReactApplicationContext

class RsAndroidSessionModule(
  reactApplicationContext: ReactApplicationContext,
) : NativeRsAndroidSessionSpec(reactApplicationContext) {
  private val appContext = reactApplicationContext.applicationContext
  private var pendingSessions: List<RsSessionService.ActiveSession> = emptyList()

  private val lifecycleObserver = object : DefaultLifecycleObserver {
    override fun onStart(owner: LifecycleOwner) {
      startDeferredSessions()
    }
  }

  private val activityListener = object : RsActivityHooks.Listener {
    override fun onNewIntent(activity: android.app.Activity, intent: Intent) {
      if (intent.getStringExtra(RsSessionService.EXTRA_NOTIFICATION_ACTION) !=
        RsSessionService.ACTION_OPEN
      ) {
        return
      }
      val sessionId = intent.getStringExtra(RsSessionService.EXTRA_SESSION_ID) ?: return
      emitNotificationAction(RsSessionService.ACTION_OPEN, sessionId)
    }
  }

  init {
    TileBridge.attach(appContext)
    PipBridge.onPipChange = { inPip -> emitPipChange(inPip) }
    TileBridge.onConnect = { hostId -> emitTileConnect(hostId) }
    ProcessLifecycleOwner.get().lifecycle.addObserver(lifecycleObserver)
    RsActivityHooks.add(activityListener)
    RsSessionService.disconnectListener = { sessionId ->
      emitNotificationAction("disconnect", sessionId)
    }
  }

  override fun getName(): String = NAME

  override fun setActiveSessions(sessions: ReadableArray) {
    val active = parseSessions(sessions)
    pendingSessions = active
    TileBridge.setSessionsRunning(active.isNotEmpty())
    RsLog.marker("NOTIFY", mapOf("sessions" to active.size, "running" to active.isNotEmpty()))
    if (active.isEmpty()) {
      appContext.stopService(Intent(appContext, RsSessionService::class.java))
      return
    }
    if (RsSessionService.isRunning()) {
      RsSessionService.updateRunningSessions(active)
    } else if (isAppForeground()) {
      startService(active)
    } else {
      RsLog.marker("NOTIFY", mapOf("start" to "deferred"))
    }
  }

  override fun setPipEligible(eligible: Boolean, aspectWidth: Double, aspectHeight: Double) {
    PipBridge.setEligible(eligible, aspectWidth.toInt(), aspectHeight.toInt())
  }

  override fun isInPip(): Boolean = PipBridge.isInPip()

  override fun setTileTarget(hostId: String, label: String) {
    TileBridge.setTarget(hostId, label)
  }

  override fun takePendingTileConnect(): String = TileBridge.takePending()

  override fun invalidate() {
    ProcessLifecycleOwner.get().lifecycle.removeObserver(lifecycleObserver)
    RsActivityHooks.remove(activityListener)
    if (RsSessionService.disconnectListener != null) {
      RsSessionService.disconnectListener = null
    }
    PipBridge.onPipChange = null
    TileBridge.onConnect = null
    pendingSessions = emptyList()
    super.invalidate()
  }

  private fun startDeferredSessions() {
    val active = pendingSessions
    if (active.isNotEmpty() && isAppForeground() && !RsSessionService.isRunning()) {
      startService(active)
    }
  }

  private fun startService(active: List<RsSessionService.ActiveSession>) {
    ContextCompat.startForegroundService(appContext, RsSessionService.intent(appContext, active))
  }

  private fun isAppForeground(): Boolean =
    ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED) ||
      RsActivityHooks.current != null

  private fun parseSessions(array: ReadableArray): List<RsSessionService.ActiveSession> {
    val result = ArrayList<RsSessionService.ActiveSession>(array.size())
    for (index in 0 until array.size()) {
      val map = array.getMap(index) ?: continue
      val sessionId = map.getString("sessionId") ?: continue
      val title = map.getString("title") ?: continue
      result += RsSessionService.ActiveSession(
        sessionId = sessionId,
        title = title,
        kind = map.getString("kind").orEmpty(),
        startedAtMs = map.getDouble("startedAtMs"),
      )
    }
    return result
  }

  private fun emitNotificationAction(action: String, sessionId: String) {
    emitOnNotificationAction(Arguments.createMap().apply {
      putString("action", action)
      putString("sessionId", sessionId)
    })
  }

  private fun emitPipChange(inPip: Boolean) {
    emitOnPipChange(Arguments.createMap().apply { putBoolean("inPip", inPip) })
  }

  private fun emitTileConnect(hostId: String) {
    emitOnTileConnect(Arguments.createMap().apply { putString("hostId", hostId) })
  }

  companion object {
    const val NAME = "RsAndroidSession"
  }
}
