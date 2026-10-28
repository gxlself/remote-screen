package ai.gxlself.remotescreen.rsnative.androidsession

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import ai.gxlself.remotescreen.MainActivity
import ai.gxlself.remotescreen.logic.androidsession.ForegroundPlan
import ai.gxlself.remotescreen.logic.androidsession.SessionRef
import ai.gxlself.remotescreen.logic.androidsession.foregroundPlan

/** Keeps an active remote session alive while the app is backgrounded. */
class RsSessionService : Service() {
  private var sessions: List<ActiveSession> = emptyList()

  override fun onCreate() {
    super.onCreate()
    instance = this
    createNotificationChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_DISCONNECT -> {
        val sessionId = intent.getStringExtra(EXTRA_SESSION_ID)
        if (!sessionId.isNullOrEmpty()) {
          disconnectListener?.invoke(sessionId)
        }
      }
      ACTION_UPDATE, null -> {
        updateSessions(readSessions(intent))
      }
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    if (instance === this) {
      instance = null
    }
    sessions = emptyList()
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private fun updateSessions(next: List<ActiveSession>) {
    sessions = next
    val plan = foregroundPlan(
      next.map { SessionRef(it.sessionId, it.startedAtMs) },
      Build.VERSION.SDK_INT,
      Build.VERSION_CODES.UPSIDE_DOWN_CAKE,
    )
    when (plan) {
      ForegroundPlan.Stop -> stopSelf()
      is ForegroundPlan.Start -> {
        val notification = buildNotification(next, plan)
        val serviceType =
          if (plan.specialUseType) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, serviceType)
      }
    }
  }

  private fun buildNotification(
    items: List<ActiveSession>,
    plan: ForegroundPlan.Start,
  ): Notification {
    val first = items.firstOrNull { it.sessionId == plan.primarySessionId } ?: items.first()
    val contentIntent = activityIntent(first.sessionId)
    val disconnectIntent = PendingIntent.getService(
      this,
      first.sessionId.hashCode(),
      Intent(this, RsSessionService::class.java).apply {
        action = ACTION_DISCONNECT
        putExtra(EXTRA_SESSION_ID, first.sessionId)
      },
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val builder = NotificationCompat.Builder(this, NotifyStrings.CHANNEL_ID)
      .setSmallIcon(applicationInfo.icon)
      .setContentTitle(NotifyStrings.CONNECTING.format(first.title))
      .setContentIntent(contentIntent)
      .setWhen(first.startedAtMs.toLong())
      .setUsesChronometer(true)
      .setOngoing(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setSilent(true)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .addAction(
        NotificationCompat.Action.Builder(
          applicationInfo.icon,
          NotifyStrings.RETURN_TO_SESSION,
          contentIntent,
        ).build(),
      )
      .addAction(
        NotificationCompat.Action.Builder(
          applicationInfo.icon,
          NotifyStrings.DISCONNECT,
          disconnectIntent,
        ).build(),
      )
    if (items.size > 1) {
      builder.setContentText(NotifyStrings.OTHER_SESSIONS.format(items.size - 1))
    }
    return builder.build()
  }

  private fun activityIntent(sessionId: String): PendingIntent = PendingIntent.getActivity(
    this,
    sessionId.hashCode(),
    Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
      putExtra(EXTRA_NOTIFICATION_ACTION, ACTION_OPEN)
      putExtra(EXTRA_SESSION_ID, sessionId)
    },
    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
  )

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
      return
    }
    val channel = NotificationChannel(
      NotifyStrings.CHANNEL_ID,
      NotifyStrings.CHANNEL_NAME,
      NotificationManager.IMPORTANCE_LOW,
    ).apply {
      setSound(null, null)
      enableVibration(false)
      setShowBadge(false)
    }
    getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }

  private fun readSessions(intent: Intent?): List<ActiveSession> {
    if (intent == null) {
      return emptyList()
    }
    val bundles =
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        intent.getParcelableArrayListExtra(EXTRA_SESSIONS, Bundle::class.java)
      } else {
        @Suppress("DEPRECATION")
        intent.getParcelableArrayListExtra<Bundle>(EXTRA_SESSIONS)
      }
    return bundles.orEmpty().mapNotNull { bundle ->
      val sessionId = bundle.getString(EXTRA_SESSION_ID)
      val title = bundle.getString(EXTRA_TITLE)
      if (sessionId.isNullOrEmpty() || title == null) {
        null
      } else {
        ActiveSession(
          sessionId = sessionId,
          title = title,
          kind = bundle.getString(EXTRA_KIND).orEmpty(),
          startedAtMs = bundle.getDouble(EXTRA_STARTED_AT_MS),
        )
      }
    }
  }

  data class ActiveSession(
    val sessionId: String,
    val title: String,
    val kind: String,
    val startedAtMs: Double,
  )

  companion object {
    const val ACTION_UPDATE = "ai.gxlself.remotescreen.UPDATE_SESSION"
    const val ACTION_DISCONNECT = "ai.gxlself.remotescreen.DISCONNECT"
    const val ACTION_OPEN = "open"
    const val EXTRA_NOTIFICATION_ACTION = "rsNotifyAction"
    const val EXTRA_SESSION_ID = "sessionId"
    private const val EXTRA_SESSIONS = "sessions"
    private const val EXTRA_TITLE = "title"
    private const val EXTRA_KIND = "kind"
    private const val EXTRA_STARTED_AT_MS = "startedAtMs"
    private const val NOTIFICATION_ID = 4201

    @Volatile private var instance: RsSessionService? = null
    @Volatile var disconnectListener: ((String) -> Unit)? = null

    fun isRunning(): Boolean = instance != null

    fun updateRunningSessions(sessions: List<ActiveSession>) {
      instance?.updateSessions(sessions)
    }

    fun intent(context: Context, sessions: List<ActiveSession>): Intent =
      Intent(context, RsSessionService::class.java).apply {
        action = ACTION_UPDATE
        putParcelableArrayListExtra(
          EXTRA_SESSIONS,
          ArrayList(sessions.map { session ->
            Bundle().apply {
              putString(EXTRA_SESSION_ID, session.sessionId)
              putString(EXTRA_TITLE, session.title)
              putString(EXTRA_KIND, session.kind)
              putDouble(EXTRA_STARTED_AT_MS, session.startedAtMs)
            }
          }),
        )
      }
  }
}
