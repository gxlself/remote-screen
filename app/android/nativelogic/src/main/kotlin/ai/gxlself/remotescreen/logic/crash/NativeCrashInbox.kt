package ai.gxlself.remotescreen.logic.crash

import java.io.File
import java.util.Locale

data class ExitRecord(
  val reason: Int,
  val timestampMs: Long,
  val pid: Int,
  val status: Int,
  val description: String?,
  val processName: String?,
  val trace: String?,
)

data class ExitSelection(val records: List<ExitRecord>, val lastHandledMs: Long?)

data class InboxFile(val name: String, val json: String)

object NativeCrashInbox {
  const val REASON_CRASH = 4
  const val REASON_CRASH_NATIVE = 5
  const val REASON_ANR = 6
  const val MAX_PER_RUN = 5
  const val KEEP_FILES = 10
  const val MAX_AGE_MS = 7L * 24 * 3600 * 1000
  const val TRACE_MAX_CHARS = 16 * 1024

  fun select(records: List<ExitRecord>, lastHandledMs: Long?, nowMs: Long): ExitSelection {
    val selected = records
      .asSequence()
      .filter { record ->
        record.reason in setOf(REASON_CRASH, REASON_CRASH_NATIVE, REASON_ANR) &&
          record.timestampMs > 0 &&
          (lastHandledMs == null || record.timestampMs > lastHandledMs) &&
          nowMs - record.timestampMs <= MAX_AGE_MS
      }
      .sortedBy { it.timestampMs }
      .toList()
      .takeLast(MAX_PER_RUN)

    val largestRecordTimestamp = records.maxOfOrNull { it.timestampMs }
    val latestHandled = when {
      largestRecordTimestamp == null -> lastHandledMs
      lastHandledMs == null -> largestRecordTimestamp
      else -> maxOf(lastHandledMs, largestRecordTimestamp)
    }
    return ExitSelection(selected, latestHandled)
  }

  fun name(record: ExitRecord): String = when (record.reason) {
    REASON_CRASH -> "JAVA_CRASH"
    REASON_CRASH_NATIVE -> {
      if (record.status > 0) {
        "NATIVE_CRASH ${signalName(record.status)}"
      } else {
        "NATIVE_CRASH"
      }
    }
    REASON_ANR -> "ANR"
    else -> "EXIT_${record.reason}"
  }

  fun signalName(signal: Int): String = when (signal) {
    1 -> "SIGHUP"
    2 -> "SIGINT"
    3 -> "SIGQUIT"
    4 -> "SIGILL"
    5 -> "SIGTRAP"
    6 -> "SIGABRT"
    7 -> "SIGBUS"
    8 -> "SIGFPE"
    9 -> "SIGKILL"
    11 -> "SIGSEGV"
    13 -> "SIGPIPE"
    15 -> "SIGTERM"
    31 -> "SIGSYS"
    else -> "SIG$signal"
  }

  fun toInbox(record: ExitRecord, appVersion: String?, osVersion: String?): InboxFile {
    val incidentAt = record.timestampMs * 1000
    val recordName = name(record)
    val message = record.description
      ?.trim()
      ?.takeIf { it.isNotEmpty() }
      ?: recordName
    val stack = if (record.reason == REASON_ANR) {
      record.trace
        ?.takeIf { it.isNotBlank() }
        ?.let { truncate(it, TRACE_MAX_CHARS) }
    } else {
      null
    }
    val thread = record.processName?.takeIf { it.isNotEmpty() }
    val json = buildString {
      append('{')
      append("\"v\":1")
      append(",\"incidentAt\":").append(incidentAt)
      append(",\"appVersion\":").append(encodeNullable(appVersion?.takeIf { it.isNotEmpty() }))
      append(",\"osVersion\":").append(encodeNullable(osVersion?.takeIf { it.isNotEmpty() }))
      append(",\"name\":").append(encodeString(recordName))
      append(",\"message\":").append(encodeString(message))
      append(",\"stack\":").append(encodeNullable(stack))
      append(",\"thread\":").append(encodeNullable(thread))
      append('}')
    }
    val filename = String.format(
      Locale.ROOT,
      "%020d-exitinfo-%d.json",
      incidentAt,
      maxOf(record.pid, 0),
    )
    return InboxFile(filename, json)
  }

  fun namesToDelete(names: List<String>, keep: Int = KEEP_FILES): List<String> {
    val jsonNames = names
      .filter { it.endsWith(".json") && !it.startsWith('.') }
      .sorted()
    return if (jsonNames.size > keep) {
      jsonNames.take(jsonNames.size - keep)
    } else {
      emptyList()
    }
  }

  fun inboxDir(dataDir: File): File = File(File(dataDir, "crashes"), "native")

  private fun truncate(value: String, max: Int): String {
    if (value.length <= max) {
      return value
    }
    val truncated = value.take(max)
    return if (truncated.lastOrNull()?.isHighSurrogate() == true) {
      truncated.dropLast(1)
    } else {
      truncated
    }
  }

  private fun encodeNullable(value: String?): String = value?.let(::encodeString) ?: "null"

  private fun encodeString(value: String): String = buildString {
    append('"')
    value.forEach { character ->
      when (character) {
        '"' -> append("\\\"")
        '\\' -> append("\\\\")
        '\n' -> append("\\n")
        '\r' -> append("\\r")
        '\t' -> append("\\t")
        else -> {
          if (character.code < 0x20) {
            append("\\u00")
            append(HEX[(character.code ushr 4) and 0x0f])
            append(HEX[character.code and 0x0f])
          } else {
            append(character)
          }
        }
      }
    }
    append('"')
  }

  private const val HEX = "0123456789abcdef"
}
