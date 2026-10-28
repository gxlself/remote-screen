package ai.gxlself.remotescreen.rsnative.terminal

import ai.gxlself.remotescreen.logic.ids.NativeId
import ai.gxlself.remotescreen.logic.terminal.TermOutputCoalescer
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsTermNative
import ai.gxlself.remotescreen.rsnative.jni.TermSink
import android.os.Handler
import android.os.Looper

/** Main-thread attachment of an RsTermNative stream to one terminal view. */
class TerminalAttachment(private val view: RsTerminalView) {
  private val mainHandler = Handler(Looper.getMainLooper())
  private val stateLock = Any()
  @Volatile private var generation = 0L
  private var requestedValue: String? = null
  private var attachedId: Long? = null
  private var activeSink: TermSink? = null
  private var retryRunnable: Runnable? = null
  private var output = TermOutputCoalescer()
  private var drainScheduled = false
  private var backlogLogged = false

  fun attach(value: String?) {
    val next = value ?: ""
    if (requestedValue == next) return
    detach()
    requestedValue = next
    if (next.isEmpty()) return
    val id = NativeId.parse(next)
    if (id == null) {
      RsLog.marker("TERMVIEW", mapOf("attach" to false, "sshId" to next))
      return
    }
    attempt(id, next, 1, generation)
  }

  fun detach() {
    synchronized(stateLock) { generation++ }
    retryRunnable?.let(mainHandler::removeCallbacks)
    retryRunnable = null
    val id = attachedId
    attachedId = null
    synchronized(stateLock) {
      activeSink = null
      output = TermOutputCoalescer()
      drainScheduled = false
      backlogLogged = false
    }
    if (id != null) {
      RsTermNative.detach(id)
    }
    requestedValue = null
    view.onTerminalDetached()
  }

  fun resize(cols: Int, rows: Int) {
    val id = attachedId ?: return
    if (cols > 0 && rows > 0) RsTermNative.resize(id, cols, rows)
  }

  private fun attempt(id: Long, value: String, number: Int, expectedGeneration: Long) {
    if (expectedGeneration != generation || requestedValue != value) return
    retryRunnable = null
    val sink = object : TermSink {
      override fun onOutput(data: ByteArray) {
        append(data, expectedGeneration, this)
      }

      override fun onClosed() {
        view.postToMain {
          if (expectedGeneration == generation) {
            RsLog.marker("TERMVIEW", mapOf("closed" to true))
          }
        }
      }
    }
    synchronized(stateLock) { activeSink = sink }
    val attached = try {
      RsTermNative.attach(id, sink)
    } catch (_: Throwable) {
      false
    }
    if (attached) {
      attachedId = id
      RsLog.marker("TERMVIEW", mapOf("attach" to true))
      view.onTerminalAttached(id)
      return
    }
    synchronized(stateLock) {
      if (activeSink === sink) {
        activeSink = null
        output = TermOutputCoalescer()
        drainScheduled = false
        backlogLogged = false
      }
    }
    if (number >= MAX_ATTEMPTS || expectedGeneration != generation) {
      RsLog.marker("TERMVIEW", mapOf("attach" to false, "sshId" to value))
      return
    }
    val retry = Runnable {
      if (expectedGeneration == generation) {
        attempt(id, value, number + 1, expectedGeneration)
      }
    }
    retryRunnable = retry
    mainHandler.postDelayed(retry, RETRY_DELAY_MS)
  }

  private fun append(data: ByteArray, expectedGeneration: Long, sink: TermSink) {
    if (data.isEmpty()) return
    val shouldSchedule = synchronized(stateLock) {
      if (expectedGeneration != generation || activeSink !== sink) return@synchronized false
      output.append(data)
      if (!drainScheduled) {
        drainScheduled = true
        true
      } else {
        false
      }
    }
    if (shouldSchedule) {
      view.post { drain(expectedGeneration) }
    }
  }

  private fun drain(expectedGeneration: Long) {
    if (expectedGeneration != generation) return
    val (data, backlogToLog) = synchronized(stateLock) {
      if (expectedGeneration != generation) return@synchronized null to null
      val backlog = output.pendingBytes
      val log = if (backlog > BACKLOG_LIMIT && !backlogLogged) {
        backlogLogged = true
        backlog
      } else {
        null
      }
      output.take(REFRESH_LIMIT) to log
    }
    if (backlogToLog != null) {
      RsLog.marker("TERMVIEW", mapOf("backlog" to backlogToLog))
    }
    if (data != null) view.feedOutput(data)

    val more = synchronized(stateLock) {
      if (expectedGeneration != generation) return@synchronized false
      val pending = output.pendingBytes > 0
      if (!pending) {
        drainScheduled = false
        backlogLogged = false
      }
      pending
    }
    if (more) view.post { drain(expectedGeneration) }
  }

  private companion object {
    const val MAX_ATTEMPTS = 10
    const val RETRY_DELAY_MS = 500L
    const val REFRESH_LIMIT = 256 * 1024
    const val BACKLOG_LIMIT = 1024 * 1024
  }
}
