package ai.gxlself.remotescreen.rsnative.androidsession.tile

import android.content.ComponentName
import android.content.Context
import android.content.SharedPreferences
import android.os.SystemClock
import android.service.quicksettings.TileService

/** Shared state between the React Native module and the Quick Settings tile. */
object TileBridge {
  private const val PREFS_NAME = "rs.tile"
  private const val HOST_ID_KEY = "hostId"
  private const val LABEL_KEY = "label"
  private const val PENDING_TIMEOUT_MS = 60_000L

  private val lock = Any()
  private var appContext: Context? = null
  private var targetHostId = ""
  private var targetLabel = ""
  private var targetSetInMemory = false
  private var sessionsRunning = false
  private var pendingHostId: String? = null
  private var pendingAtElapsedMs = 0L

  @Volatile
  var onConnect: ((String) -> Unit)? = null

  fun attach(ctx: Context) {
    synchronized(lock) {
      val applicationContext = ctx.applicationContext
      if (appContext === applicationContext) {
        return
      }
      appContext = applicationContext
      val preferences = applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
      if (!targetSetInMemory) {
        targetHostId = preferences.getString(HOST_ID_KEY, "").orEmpty()
        targetLabel = preferences.getString(LABEL_KEY, "").orEmpty()
      } else {
        persistTargetLocked(preferences)
      }
    }
  }

  fun setTarget(hostId: String, label: String) {
    val context = synchronized(lock) {
      targetHostId = hostId
      targetLabel = if (hostId.isEmpty()) "" else label
      targetSetInMemory = true
      appContext?.also { context ->
        persistTargetLocked(context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE))
      }
    }
    context?.let(::requestListeningState)
  }

  fun takePending(): String {
    synchronized(lock) {
      val hostId = pendingHostId ?: return ""
      pendingHostId = null
      val age = SystemClock.elapsedRealtime() - pendingAtElapsedMs
      pendingAtElapsedMs = 0L
      return if (age > PENDING_TIMEOUT_MS) "" else hostId
    }
  }

  fun setSessionsRunning(running: Boolean) {
    val context = synchronized(lock) {
      val changed = sessionsRunning != running
      sessionsRunning = running
      if (changed) appContext else null
    }
    context?.let(::requestListeningState)
  }

  internal fun request(hostId: String) {
    val listener = onConnect
    if (listener != null) {
      listener.invoke(hostId)
      return
    }
    synchronized(lock) {
      pendingHostId = hostId
      pendingAtElapsedMs = SystemClock.elapsedRealtime()
    }
  }

  fun hasRunningSessions(): Boolean = synchronized(lock) { sessionsRunning }

  fun target(): Target = synchronized(lock) {
    Target(hostId = targetHostId, label = targetLabel)
  }

  private fun persistTargetLocked(preferences: SharedPreferences) {
    preferences.edit().apply {
      if (targetHostId.isEmpty()) {
        remove(HOST_ID_KEY)
        remove(LABEL_KEY)
      } else {
        putString(HOST_ID_KEY, targetHostId)
        putString(LABEL_KEY, targetLabel)
      }
    }.apply()
  }

  private fun requestListeningState(context: Context) {
    TileService.requestListeningState(
      context,
      ComponentName(context, RsTileService::class.java),
    )
  }

  data class Target(val hostId: String, val label: String)
}
