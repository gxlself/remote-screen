package ai.gxlself.remotescreen.rsnative.files

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import ai.gxlself.remotescreen.MainActivity
import ai.gxlself.remotescreen.R

/** Keeps file transfers alive while the app is backgrounded. */
class RsTransferService : Service() {
  override fun onCreate() {
    super.onCreate()
    createNotificationChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
      stopSelf()
      return START_NOT_STICKY
    }

    startForegroundServiceNotification()
    return START_NOT_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onTimeout(startId: Int, fgsType: Int) {
    // Android 15 limits background dataSync execution time.
    ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
    stopSelf()
  }

  private fun startForegroundServiceNotification() {
    val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
    } else {
      0
    }
    ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(), type)
  }

  private fun notification(): Notification {
    val contentIntent = PendingIntent.getActivity(
      this,
      NOTIFICATION_ID,
      Intent(this, MainActivity::class.java).apply {
        flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
      },
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.rs_transfer)
      .setContentTitle("正在传输文件")
      .setContentIntent(contentIntent)
      .setOngoing(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setSilent(true)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .build()
  }

  private fun createNotificationChannel() {
    val channel = NotificationChannel(
      CHANNEL_ID,
      "文件传输",
      NotificationManager.IMPORTANCE_LOW,
    ).apply {
      setSound(null, null)
      enableVibration(false)
      setShowBadge(false)
    }
    getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }

  companion object {
    const val ACTION_START = "ai.gxlself.remotescreen.files.START"
    const val ACTION_STOP = "ai.gxlself.remotescreen.files.STOP"
    private const val CHANNEL_ID = "rs.files.transfer"
    private const val NOTIFICATION_ID = 4202

    fun startIntent(context: Context): Intent =
      Intent(context, RsTransferService::class.java).setAction(ACTION_START)

    fun stopIntent(context: Context): Intent =
      Intent(context, RsTransferService::class.java).setAction(ACTION_STOP)
  }
}
