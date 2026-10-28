package ai.gxlself.remotescreen.rsnative.session

import android.os.Handler
import android.os.Looper
import ai.gxlself.remotescreen.logic.adapters.AdapterIds
import ai.gxlself.remotescreen.logic.ids.NativeId
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsRawNative
import ai.gxlself.remotescreen.rsnative.jni.RsSessionNative
import ai.gxlself.remotescreen.rsnative.session.video.SessionVideoPipeline

/** Main-thread lifecycle bridge between a view and the native session (or adapter raw) sink. */
class SessionAttachment(
  private val pipelineFactory: () -> SessionVideoPipeline,
  private val onAttachedChanged: (Long?) -> Unit = {},
) {
  private val handler = Handler(Looper.getMainLooper())
  private var generation = 0L
  private var retry: Runnable? = null
  private var attachedId: Long? = null
  private var attachedRaw = false
  private var pipeline: SessionVideoPipeline? = null

  fun attach(value: String) {
    checkMainThread()
    detach()
    if (value.isEmpty()) return
    val id = NativeId.parse(value)
    if (id == null) {
      RsLog.marker("SESSVIEW", mapOf("error" to "bad-session-id"))
      return
    }
    val activePipeline = pipelineFactory().also { pipeline = it }
    attempt(id, value, activePipeline, number = 1, expectedGeneration = generation)
  }

  fun detach() {
    checkMainThread()
    generation += 1
    retry?.let(handler::removeCallbacks)
    retry = null
    val id = attachedId
    val raw = attachedRaw
    attachedId = null
    attachedRaw = false
    if (id != null) {
      if (raw) RsRawNative.detach(id) else RsSessionNative.detach(id)
    }
    pipeline?.stop()
    onAttachedChanged(null)
  }

  private fun attempt(
    id: Long,
    value: String,
    activePipeline: SessionVideoPipeline,
    number: Int,
    expectedGeneration: Long,
  ) {
    checkMainThread()
    if (expectedGeneration != generation) return
    retry = null
    val raw = AdapterIds.isAdapterId(id)
    if (raw) activePipeline.startRaw(id) else activePipeline.start(id)
    val attached = try {
      if (raw) RsRawNative.attach(id, activePipeline) else RsSessionNative.attach(id, activePipeline)
    } catch (_: RuntimeException) {
      false
    }
    if (attached) {
      attachedId = id
      attachedRaw = raw
      onAttachedChanged(id)
      RsLog.marker("SESSVIEW", marker(raw, "attach" to true, "session" to value))
      return
    }

    activePipeline.stop()
    RsLog.marker("SESSVIEW", marker(raw, "attach" to false, "session" to value, "attempt" to number))
    if (number >= MAX_ATTEMPTS || expectedGeneration != generation) return
    val next = Runnable {
      if (expectedGeneration == generation) {
        attempt(id, value, activePipeline, number + 1, expectedGeneration)
      }
    }
    retry = next
    handler.postDelayed(next, RETRY_DELAY_MS)
  }

  private fun marker(raw: Boolean, vararg fields: Pair<String, Any?>): Map<String, Any?> =
    if (raw) mapOf(*fields, "raw" to true) else mapOf(*fields)

  private fun checkMainThread() {
    check(Looper.myLooper() === Looper.getMainLooper()) { "session attachment requires main thread" }
  }

  companion object {
    private const val MAX_ATTEMPTS = 10
    private const val RETRY_DELAY_MS = 500L
  }
}
