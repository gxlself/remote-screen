package ai.gxlself.remotescreen.rsnative.session.video

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.SystemClock
import android.view.Surface
import ai.gxlself.remotescreen.logic.session.AnnexB
import ai.gxlself.remotescreen.logic.session.DecoderAction
import ai.gxlself.remotescreen.logic.session.DecoderChoice
import ai.gxlself.remotescreen.logic.session.FrameMeta
import ai.gxlself.remotescreen.logic.session.Latency
import ai.gxlself.remotescreen.logic.session.SessionCodec
import ai.gxlself.remotescreen.logic.session.decoderAction
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.nio.ByteBuffer

internal data class PendingFrame(
  val avcc: ByteArray, val codec: Int, val vps: ByteArray?, val sps: ByteArray?, val pps: ByteArray?,
  val receivedUs: Long, val captureLocalUs: Long, val clockSynced: Boolean,
)

internal data class DecodeFrame(
  val meta: FrameMeta, val payload: PendingFrame, val width: Int, val height: Int,
)

/** Confined to rs-decoder, including construction, callbacks and release. Owns no Surface. */
internal class SessionDecoder(private val handler: Handler, private val listener: Listener) {
  interface Listener {
    fun isActive(): Boolean
    fun isCurrent(epoch: Int): Boolean
    fun nextFrame(): DecodeFrame?
    fun decoderReady(epoch: Int?, ready: Boolean)
    fun decodeFailed(epoch: Int?, droppedCurrent: Boolean)
    fun resetForSurface(requestKeyframe: Boolean)
    fun onVideoSize(epoch: Int, width: Int, height: Int)
    fun onDecoded(epoch: Int, width: Int, height: Int, decodeUs: Long, latencyUs: Long?)
  }

  private data class Timing(
    val epoch: Int, val queuedAtUs: Long, val receivedUs: Long,
    val captureLocalUs: Long, val clockSynced: Boolean,
  )

  private var codec: MediaCodec? = null
  private var surface: Surface? = null
  private var epoch: Int? = null
  private var codecId = 0
  private var name = ""
  private var mime = ""
  private var width = 0
  private var height = 0
  private var lowLatency = false
  private var parameterSets: List<ByteArray>? = null
  private val inputBuffers = ArrayDeque<Int>()
  private val timings = linkedMapOf<Long, Timing>()
  // Only the keyframe used to configure/reconfigure can wait outside the bounded gate.
  private var staged: DecodeFrame? = null

  fun setSurface(next: Surface?) {
    if (surface === next) return
    surface = next
    if (next == null) {
      release()
      listener.resetForSurface(requestKeyframe = false)
      return
    }
    val current = codec
    if (current != null && Build.VERSION.SDK_INT >= 23) {
      try {
        current.setOutputSurface(next)
        pump()
        return
      } catch (error: RuntimeException) {
        logError("surface", error)
      }
    }
    if (current != null) marker("rebuild")
    release()
    listener.resetForSurface(requestKeyframe = true)
    pump()
  }

  fun pump() {
    if (!listener.isActive()) return
    if (epoch?.let { !listener.isCurrent(it) } == true) {
      marker("rebuild")
      release()
    }
    if (surface?.isValid != true) return
    while (listener.isActive()) {
      if (codec != null && inputBuffers.isEmpty()) return
      val frame = staged?.also { staged = null } ?: listener.nextFrame() ?: return
      if (!listener.isCurrent(frame.meta.epoch)) continue
      try {
        val payload = frame.payload
        val nextMime = requireNotNull(SessionCodec.mime(payload.codec)) { "unsupported codec" }
        val sets = if (frame.meta.keyframe && frame.meta.hasParameterSets) {
          val sps = requireNotNull(payload.sps)
          val pps = requireNotNull(payload.pps)
          require(sps.isNotEmpty() && pps.isNotEmpty()) { "empty parameter sets" }
          AnnexB.csd(payload.codec, payload.vps, sps, pps)
        } else {
          null
        }
        val setsChanged = sets?.let { !AnnexB.sameParameterSets(parameterSets, it) }
        val action = decoderAction(
          hasCodec = codec != null,
          currentEpoch = epoch,
          frameEpoch = frame.meta.epoch,
          currentCodec = codecId,
          frameCodec = payload.codec,
          keyframe = frame.meta.keyframe,
          setsChanged = setsChanged,
        )
        if (action != DecoderAction.Feed) {
          if (action == DecoderAction.Rebuild) marker("rebuild")
          release()
          create(frame, nextMime, checkNotNull(sets))
          staged = frame
          // Input callbacks from start() will feed this same, still-AVCC keyframe.
          return
        }
        val current = checkNotNull(codec)
        val index = inputBuffers.removeFirst()
        require(payload.avcc.isNotEmpty() && AnnexB.avccToAnnexBInPlace(payload.avcc)) {
          "invalid AVCC"
        }
        val buffer = checkNotNull(current.getInputBuffer(index))
        buffer.clear()
        require(buffer.remaining() >= payload.avcc.size) { "input buffer too small" }
        buffer.put(payload.avcc)
        // Local decode duration uses the monotonic clock; ABI capture/receive timestamps stay Unix us.
        val timing = Timing(
          frame.meta.epoch, SystemClock.elapsedRealtimeNanos() / 1_000,
          payload.receivedUs, payload.captureLocalUs, payload.clockSynced,
        )
        current.queueInputBuffer(
          index, 0, payload.avcc.size, payload.receivedUs,
          if (frame.meta.keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0,
        )
        timings[payload.receivedUs] = timing
        if (timings.size > 32) timings.remove(timings.keys.first())
      } catch (error: Exception) {
        fail("input", error, frame.meta.epoch, droppedCurrent = true)
        return
      }
    }
  }

  private fun create(frame: DecodeFrame, nextMime: String, sets: List<ByteArray>) {
    val candidate = checkNotNull(CodecPicker.pick(nextMime)) { "no decoder" }
    name = candidate.name
    mime = nextMime
    width = frame.width
    height = frame.height
    epoch = frame.meta.epoch
    codecId = frame.payload.codec
    parameterSets = sets
    val keys = DecoderChoice.formatKeys(name, Build.VERSION.SDK_INT)
    try {
      configure(sets, keys)
    } catch (error: Exception) {
      logError("configure-standard-retry", error)
      // A failed configure may leave vendor state unusable. Retry on a fresh codec.
      disposeCodec()
      configure(sets, keys.filterKeys { !it.startsWith("vendor.") })
    }
    checkNotNull(codec).start()
    listener.decoderReady(epoch, true)
    marker("create")
  }

  private fun configure(sets: List<ByteArray>, keys: Map<String, Int>) {
    val format = MediaFormat.createVideoFormat(mime, width, height)
    sets.forEachIndexed { index, bytes -> format.setByteBuffer("csd-$index", ByteBuffer.wrap(bytes)) }
    keys.forEach { (key, value) -> format.setInteger(key, value) }
    lowLatency = keys["low-latency"] == 1 || keys.keys.any { it.startsWith("vendor.") }
    val created = MediaCodec.createByCodecName(name)
    codec = created
    created.setCallback(callback, handler)
    created.configure(format, checkNotNull(surface), null, 0)
  }

  private val callback = object : MediaCodec.Callback() {
    override fun onInputBufferAvailable(source: MediaCodec, index: Int) {
      if (codec !== source || !listener.isActive()) return
      inputBuffers.addLast(index)
      pump()
    }

    override fun onOutputBufferAvailable(source: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
      if (codec !== source || !listener.isActive()) return
      val timing = timings.remove(info.presentationTimeUs)
      val currentEpoch = epoch
      val render = currentEpoch != null && listener.isCurrent(currentEpoch) &&
        info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0
      try {
        // No presentation timer or jitter buffer: hand the image to Surface immediately.
        source.releaseOutputBuffer(index, render)
        if (render && timing != null && listener.isCurrent(timing.epoch)) {
          val decodeUs = (SystemClock.elapsedRealtimeNanos() / 1_000 - timing.queuedAtUs).coerceAtLeast(0)
          val latencyUs = Latency.latencyUs(
            timing.clockSynced, timing.captureLocalUs, System.currentTimeMillis() * 1_000,
          )
          listener.onDecoded(timing.epoch, width, height, decodeUs, latencyUs)
        }
      } catch (error: RuntimeException) {
        fail("output", error, currentEpoch, droppedCurrent = true)
      }
    }

    override fun onOutputFormatChanged(source: MediaCodec, format: MediaFormat) {
      if (codec !== source || !listener.isActive()) return
      val currentEpoch = epoch ?: return
      if (!listener.isCurrent(currentEpoch)) return
      try {
        width = croppedSize(format, "crop-left", "crop-right", MediaFormat.KEY_WIDTH, width)
        height = croppedSize(format, "crop-top", "crop-bottom", MediaFormat.KEY_HEIGHT, height)
        listener.onVideoSize(currentEpoch, width, height)
      } catch (error: RuntimeException) {
        fail("format", error, currentEpoch, droppedCurrent = false)
      }
    }

    override fun onError(source: MediaCodec, error: MediaCodec.CodecException) {
      if (codec !== source || !listener.isActive()) return
      if (error.isTransient) {
        logError("transient", error)
      } else {
        fail("codec", error, epoch, droppedCurrent = false)
      }
    }
  }

  private fun croppedSize(format: MediaFormat, start: String, end: String, size: String, fallback: Int): Int {
    val value = if (format.containsKey(start) && format.containsKey(end)) {
      format.getInteger(end) - format.getInteger(start) + 1
    } else if (format.containsKey(size)) {
      format.getInteger(size)
    } else {
      fallback
    }
    require(value > 0) { "invalid output dimensions" }
    return value
  }

  private fun fail(stage: String, error: Exception, failedEpoch: Int?, droppedCurrent: Boolean) {
    logError(stage, error)
    release()
    listener.decodeFailed(failedEpoch, droppedCurrent)
  }

  fun release() {
    val previousEpoch = epoch
    disposeCodec()
    staged = null
    parameterSets = null
    epoch = null
    listener.decoderReady(previousEpoch, false)
  }

  private fun disposeCodec() {
    val previous = codec
    // Invalidate first so queued callbacks from this instance cannot touch new state.
    codec = null
    inputBuffers.clear()
    timings.clear()
    if (previous == null) return
    try {
      previous.stop()
    } catch (error: RuntimeException) {
      logError("stop", error)
    } finally {
      try {
        previous.release()
      } catch (error: RuntimeException) {
        logError("release", error)
      }
      marker("release")
    }
  }

  private fun marker(action: String) = RsLog.marker("SESSVIEW", mapOf(
    "decoder" to action, "name" to name, "mime" to mime,
    "w" to width, "h" to height, "lowLatency" to lowLatency,
  ))

  private fun logError(stage: String, error: Exception) = RsLog.marker("SESSVIEW", mapOf(
    "error" to stage, "type" to error.javaClass.simpleName, "name" to name,
  ))
}
