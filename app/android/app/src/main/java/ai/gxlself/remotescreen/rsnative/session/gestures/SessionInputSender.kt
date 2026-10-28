package ai.gxlself.remotescreen.rsnative.session.gestures

import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.CloseOnce
import ai.gxlself.remotescreen.logic.gestures.PointerThrottle
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsSessionNative
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.Executors
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit

/**
 * Serializes all session input JNI calls away from the Android UI thread.
 *
 * The session id is read only on the input executor.  The view owns the id and
 * may clear it while a queued input item is waiting; that makes stale events a
 * no-op instead of allowing them to reach a newly attached session.
 */
class SessionInputSender(private val sessionId: () -> Long?) {
  private val executor: ScheduledExecutorService =
    Executors.newSingleThreadScheduledExecutor(InputThreadFactory)
  private var throttle = PointerThrottle()
  private var pointerFlush: ScheduledFuture<*>? = null
  private var generation = 0L
  private var closed = false
  private var sessionBound = false
  private var boundSession: Long? = null
  private val closeOnce = CloseOnce {
    synchronized(this) {
      closed = true
      generation += 1
      pointerFlush?.cancel(false)
      pointerFlush = null
    }
    executor.shutdown()
  }

  /** Binds lifecycle state before the next input item is enqueued. */
  fun setSession(id: Long?) {
    execute {
      val next = id?.takeIf { it != 0L }
      if (sessionBound && boundSession == next) return@execute
      sessionBound = true
      boundSession = next
      pointerFlush?.cancel(false)
      pointerFlush = null
      throttle = PointerThrottle()
      generation += 1
    }
  }

  fun pointer(point: GPoint) {
    execute {
      val id = currentSession() ?: return@execute
      val now = nowSeconds()
      throttle.offer(point, now)?.let { sendPointer(id, it) }
      schedulePointerFlush(id, now)
    }
  }

  /** Sends the latest pending pointer immediately at the end of a gesture. */
  fun flushPointer() {
    execute {
      val id = currentSession() ?: return@execute
      pointerFlush?.cancel(false)
      pointerFlush = null
      throttle.flush(nowSeconds())?.let { sendPointer(id, it) }
    }
  }

  fun button(button: Int, down: Boolean, point: GPoint, clicks: Int = 1) {
    execute {
      val id = currentSession() ?: return@execute
      RsSessionNative.button(id, button, down, point.x, point.y, clicks)
    }
  }

  fun click(button: Int, point: GPoint, clicks: Int = 1) {
    execute {
      val id = currentSession() ?: return@execute
      RsSessionNative.button(id, button, true, point.x, point.y, clicks)
      RsSessionNative.button(id, button, false, point.x, point.y, clicks)
    }
  }

  fun scroll(dx: Float, dy: Float, phase: Int = PHASE_NONE, momentum: Int = PHASE_NONE) {
    execute {
      val id = currentSession() ?: return@execute
      RsSessionNative.scroll(id, dx, dy, phase, momentum)
    }
  }

  fun releaseAll() {
    execute {
      pointerFlush?.cancel(false)
      pointerFlush = null
      throttle = PointerThrottle()
      val id = currentSession() ?: return@execute
      RsSessionNative.releaseAll(id)
    }
  }

  fun centerCursor() {
    execute {
      RsLog.marker("SESSVIEW", mapOf("command" to "centerCursor", "impl" to true))
      val id = currentSession() ?: return@execute
      RsSessionNative.pointer(id, 0.5f, 0.5f)
    }
  }

  /** Stops accepting new work and lets already queued calls drain in order. */
  fun close() {
    closeOnce.close()
  }

  private fun sendPointer(id: Long, point: GPoint) {
    RsSessionNative.pointer(id, point.x, point.y)
  }

  private fun schedulePointerFlush(id: Long, now: Double) {
    if (!throttle.hasPending || pointerFlush?.isDone == false) return
    val last = throttle.lastEmissionTimeSec ?: now
    val delaySec = (last + PointerThrottle.INTERVAL_SEC - now).coerceAtLeast(0.0)
    val expectedGeneration = generation
    pointerFlush = executor.schedule({
      if (expectedGeneration == generation) {
        pointerFlush = null
        if (currentSession() == id) {
          val flushed = throttle.flush(nowSeconds())
          if (flushed != null) {
            sendPointer(id, flushed)
          } else if (throttle.hasPending) {
            schedulePointerFlush(id, nowSeconds())
          }
        }
      }
    }, (delaySec * 1_000_000_000.0).toLong(), TimeUnit.NANOSECONDS)
  }

  private fun currentSession(): Long? = if (sessionBound) boundSession else sessionId()?.takeIf { it != 0L }

  private fun execute(block: () -> Unit) {
    synchronized(this) {
      if (closed || closeOnce.isClosed) return
      executor.execute {
        try {
          block()
        } catch (_: RuntimeException) {
          // A session can disappear between attachment and an enqueued call.
          // Native lifecycle code owns the detailed error reporting.
        }
      }
    }
  }

  private fun nowSeconds(): Double = System.nanoTime() / 1_000_000_000.0

  private object InputThreadFactory : ThreadFactory {
    override fun newThread(runnable: Runnable): Thread =
      Thread(runnable, "rs-input").apply { isDaemon = true }
  }

  companion object {
    private const val PHASE_NONE = 0
  }
}
