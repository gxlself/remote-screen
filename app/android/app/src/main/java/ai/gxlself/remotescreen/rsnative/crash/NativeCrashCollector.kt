package ai.gxlself.remotescreen.rsnative.crash

import android.app.ActivityManager
import android.app.Application
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.annotation.RequiresApi
import ai.gxlself.remotescreen.logic.crash.ExitRecord
import ai.gxlself.remotescreen.logic.crash.NativeCrashInbox
import ai.gxlself.remotescreen.logic.launch.LaunchDataDir
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.io.File
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicBoolean

object NativeCrashCollector {
  private const val PREFERENCES = "rs.crash"
  private const val LAST_HANDLED_KEY = "exitinfo.lastHandledMs"
  private const val MAX_EXIT_REASONS = 16
  private const val TRACE_MAX_BYTES = 64 * 1024
  private val started = AtomicBoolean(false)

  fun start(app: Application) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R ||
      !started.compareAndSet(false, true)
    ) {
      return
    }
    Thread({
      try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
          collect(app)
        }
      } catch (error: Throwable) {
        RsLog.marker(
          "CRASH",
          mapOf(
            "source" to "exitinfo",
            "error" to (error.message ?: error::class.java.simpleName),
          ),
        )
      }
    }, "rs-crash-exit").apply {
      isDaemon = true
    }.start()
  }

  @RequiresApi(Build.VERSION_CODES.R)
  private fun collect(app: Application) {
    val activityManager = app.getSystemService(ActivityManager::class.java) ?: return
    val infos = activityManager.getHistoricalProcessExitReasons(null, 0, MAX_EXIT_REASONS)
    val records = infos.map { info ->
      ExitRecord(
        reason = info.reason,
        timestampMs = info.timestamp,
        pid = info.pid,
        status = info.status,
        description = info.description,
        processName = info.processName,
        trace = null,
      )
    }
    val preferences = app.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    val lastHandledMs = if (preferences.contains(LAST_HANDLED_KEY)) {
      preferences.getLong(LAST_HANDLED_KEY, 0)
    } else {
      null
    }
    val selection = NativeCrashInbox.select(records, lastHandledMs, System.currentTimeMillis())
    val selected = selection.records.map { record ->
      if (record.reason != NativeCrashInbox.REASON_ANR) {
        record
      } else {
        val info = infos.firstOrNull { info ->
          info.timestamp == record.timestampMs && info.pid == record.pid
        }
        record.copy(trace = info?.let(::readTrace))
      }
    }

    var written = 0
    var failed = 0
    if (selected.isNotEmpty()) {
      val directory = NativeCrashInbox.inboxDir(LaunchDataDir.dir(app.noBackupFilesDir))
      directory.mkdirs()
      val appVersion = currentAppVersion(app)
      for (record in selected) {
        val inbox = NativeCrashInbox.toInbox(
          record,
          appVersion,
          Build.VERSION.SDK_INT.toString(),
        )
        val temporary = File(directory, ".tmp-${inbox.name}")
        val destination = File(directory, inbox.name)
        try {
          temporary.writeText(inbox.json, Charsets.UTF_8)
          if (temporary.renameTo(destination)) {
            written += 1
          } else {
            temporary.delete()
            failed += 1
          }
        } catch (_: Throwable) {
          temporary.delete()
          failed += 1
        }
      }
      directory.list()
        ?.let { names -> NativeCrashInbox.namesToDelete(names.toList()) }
        ?.forEach { name -> File(directory, name).delete() }
    }

    val newLastHandledMs = selection.lastHandledMs
    if (newLastHandledMs != null && newLastHandledMs != lastHandledMs) {
      preferences.edit().putLong(LAST_HANDLED_KEY, newLastHandledMs).apply()
    }
    if (selected.isNotEmpty()) {
      RsLog.marker(
        "CRASH",
        mapOf("source" to "exitinfo", "written" to written, "failed" to failed),
      )
    }
  }

  @RequiresApi(Build.VERSION_CODES.R)
  private fun readTrace(info: ApplicationExitInfo): String? = try {
    info.traceInputStream?.use { input ->
      val bytes = ByteArray(TRACE_MAX_BYTES)
      var length = 0
      while (length < bytes.size) {
        val count = input.read(bytes, length, bytes.size - length)
        if (count <= 0) {
          break
        }
        length += count
      }
      String(bytes, 0, length, StandardCharsets.UTF_8)
    }
  } catch (_: Throwable) {
    null
  }

  private fun currentAppVersion(app: Application): String? = try {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      app.packageManager
        .getPackageInfo(app.packageName, PackageManager.PackageInfoFlags.of(0L))
        .versionName
    } else {
      legacyAppVersion(app)
    }
  } catch (_: Throwable) {
    null
  }

  @Suppress("DEPRECATION")
  private fun legacyAppVersion(app: Application): String? =
    app.packageManager.getPackageInfo(app.packageName, 0).versionName
}
