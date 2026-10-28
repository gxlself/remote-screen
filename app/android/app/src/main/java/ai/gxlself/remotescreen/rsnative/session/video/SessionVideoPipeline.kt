package ai.gxlself.remotescreen.rsnative.session.video

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.view.Surface
import ai.gxlself.remotescreen.logic.gestures.GRect
import ai.gxlself.remotescreen.logic.session.DecodeStats
import ai.gxlself.remotescreen.logic.session.EpochChange
import ai.gxlself.remotescreen.logic.session.EpochTracker
import ai.gxlself.remotescreen.logic.session.FrameMeta
import ai.gxlself.remotescreen.logic.session.GateAction
import ai.gxlself.remotescreen.logic.session.SessionFrameGate
import ai.gxlself.remotescreen.logic.session.VideoStatsSnapshot
import ai.gxlself.remotescreen.logic.viewport.ViewportGeometry
import ai.gxlself.remotescreen.rsnative.adapters.RawFrameRenderer
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RawSink
import ai.gxlself.remotescreen.rsnative.jni.RsSessionNative
import ai.gxlself.remotescreen.rsnative.jni.SessionSink

/**
 * One pipeline per session view. start/stop/setSurface belong to the main thread;
 * detach the native sink before stop. This class never attaches/detaches a session.
 * The ingress lock protects only metadata and the bounded gate, never MediaCodec/JNI/UI work.
 * startRaw is the adapter-session mode: RawFrameRenderer pulls and draws frames on rs-raw;
 * there is no MediaCodec or gate, and rs-decoder only runs events, stats and the timer.
 */
class SessionVideoPipeline(private val sink: SessionEventSink) : RawSink {
  private val main = Handler(Looper.getMainLooper())
  private val lock = Any()
  @Volatile private var current: Run? = null
  private var surface: Surface? = null // Main-thread state, retained across start/stop.

  internal interface GeometrySink {
    fun onStreamCrop(
      epoch: Int,
      srcX: Float,
      srcY: Float,
      srcW: Float,
      srcH: Float,
      displayW: Int,
      displayH: Int,
    )

    fun onVideoFrameRendered(epoch: Int)
  }

  fun start(sessionId: Long) {
    checkMainThread()
    check(current == null) { "detach and stop the previous pipeline before start" }
    val thread = HandlerThread("rs-decoder").apply { start() }
    val run = Run(sessionId, thread, Handler(thread.looper))
    val initialSurface = surface
    synchronized(lock) {
      current = run
      // Enqueue initialization before attach can replay config/frame callbacks.
      run.handler.post {
        if (!run.isActive()) return@post
        run.decoder = SessionDecoder(run.handler, run)
        run.decoder?.setSurface(initialSurface)
        run.handler.postDelayed(run.timer, 100)
      }
    }
  }

  /** Adapter session (raw frames). Same preconditions as start. */
  fun startRaw(sessionId: Long) {
    checkMainThread()
    check(current == null) { "detach and stop the previous pipeline before start" }
    val thread = HandlerThread("rs-decoder").apply { start() }
    val run = Run(sessionId, thread, Handler(thread.looper), raw = true)
    run.renderer?.setSurface(surface)
    synchronized(lock) {
      current = run
      run.handler.postDelayed(run.timer, 100)
    }
  }

  /** Caller must first detach; shutdown is ordered on rs-decoder without blocking main. */
  fun stop() {
    checkMainThread()
    val run = synchronized(lock) {
      val active = current ?: return
      current = null
      active.accepting = false
      if (!active.raw) {
        active.gate.reset(waitForKeyframe = false, nowMs = SystemClock.elapsedRealtime())
        active.gate.decoderReady(false)
      }
      active
    }
    run.renderer?.release()
    run.shutdown()
  }

  @Suppress("UNUSED_PARAMETER") // Surface bounds are view pixels; codec dimensions come from StreamConfig.
  fun setSurface(surface: Surface?, width: Int, height: Int) {
    checkMainThread()
    this.surface = surface
    val run = current ?: return
    run.renderer?.let {
      it.setSurface(surface)
      return
    }
    run.handler.post {
      if (run.isActive()) run.decoder?.setSurface(surface)
    }
  }

  /** Called on the main thread once the view has laid out an epoch's geometry. */
  fun geometryReady(epoch: Int) {
    checkMainThread()
    synchronized(lock) {
      val run = current?.takeIf { it.accepting } ?: return
      if (run.geometryAwaitingEpoch != epoch) return
      run.geometryAwaitingEpoch = null
      run.geometryReadyEpoch = epoch
      run.handler.removeCallbacks(run.geometryTimeout)
      run.feedLocked()
    }
  }

  override fun onStreamCrop(
    epoch: Int,
    srcX: Float,
    srcY: Float,
    srcW: Float,
    srcH: Float,
    displayW: Int,
    displayH: Int,
  ) {
    synchronized(lock) {
      val run = current?.takeIf { it.accepting } ?: return
      val next = geometry(srcX, srcY, srcW, srcH, displayW, displayH)
      run.latestGeometryEpoch = epoch
      run.streamGeometry[epoch] = next
      if (next.src != run.displayedGeometry.src) {
        run.geometryAwaitingEpoch = epoch
        run.geometryReadyEpoch = null
        run.handler.removeCallbacks(run.geometryTimeout)
        run.geometryTimeout = Runnable {
          synchronized(lock) {
            if (run.isActive() && run.geometryAwaitingEpoch == epoch) {
              run.geometryAwaitingEpoch = null
              run.geometryReadyEpoch = epoch
              run.feedLocked()
            }
          }
        }
        run.handler.postDelayed(run.geometryTimeout, GEOMETRY_TIMEOUT_MS)
      } else {
        run.geometryAwaitingEpoch = null
        run.handler.removeCallbacks(run.geometryTimeout)
        run.geometryReadyEpoch = epoch
      }
      main.post {
        if (!run.isActive() || run.latestGeometryEpoch != epoch) return@post
        val geometrySink = sink as? GeometrySink ?: return@post
        geometrySink.onStreamCrop(
          epoch, next.src.x, next.src.y, next.src.w, next.src.h,
          next.displayW, next.displayH,
        )
      }
      while (run.streamGeometry.size > 8) run.streamGeometry.remove(run.streamGeometry.keys.first())
    }
  }

  override fun onRawReady(seq: Long, width: Int, height: Int) {
    val renderer = synchronized(lock) { current?.takeIf { it.accepting }?.renderer } ?: return
    renderer.onReady(seq, width, height)
  }

  override fun onFrame(
    frameId: Int, keyframe: Boolean, displayId: Int, epoch: Int, codec: Int,
    clockSynced: Boolean, captureTsUs: Long, captureLocalUs: Long, receivedUs: Long,
    vps: ByteArray?, sps: ByteArray?, pps: ByteArray?, avcc: ByteArray,
  ) {
    synchronized(lock) {
      val run = current?.takeIf { it.accepting && !it.raw } ?: return
      val configuredEpoch = run.configEpoch
      if (configuredEpoch != null && (epoch and 0xff) != (configuredEpoch and 0xff)) {
        run.extraDropped++
        return
      }
      val fullEpoch = configuredEpoch ?: (epoch and 0xff)
      run.observeEpochLocked(fullEpoch)
      val meta = FrameMeta(
        frameId, fullEpoch, keyframe, sps != null && pps != null && (codec != 1 || vps != null),
      )
      // JNI already copied these arrays. Retain them without another full-frame allocation.
      val payload = PendingFrame(avcc, codec, vps, sps, pps, receivedUs, captureLocalUs, clockSynced)
      run.postActionsLocked(run.gate.push(meta, payload, SystemClock.elapsedRealtime()))
      run.feedLocked()
    }
  }

  override fun onEvent(
    kind: Int, width: Int, height: Int, fps: Int, displayId: Int, epoch: Int, codec: Int,
    cursorVisible: Boolean, cursorX: Float, cursorY: Float, rttUs: Long, offsetUs: Long,
    code: Int, shapeId: Long, hotX: Int, hotY: Int, scale: Float,
    data: ByteArray?, message: ByteArray?,
  ) {
    synchronized(lock) {
      val run = current?.takeIf { it.accepting } ?: return
      when (kind) {
        SessionSink.EV_STREAM_CONFIG -> {
          val streamWidth = width.takeIf { it > 0 } ?: 1920
          val streamHeight = height.takeIf { it > 0 } ?: 1080
          run.configEpoch = epoch
          run.configWidth = streamWidth
          run.configHeight = streamHeight
          run.configCodec = codec
          run.observeEpochLocked(epoch)
          run.handler.post {
            if (!run.isCurrent(epoch)) return@post
            run.onVideoSize(epoch, streamWidth, streamHeight)
            run.videoCodec = codec
          }
          run.feedLocked()
          // Config also seeds the cached cursor visibility/position, as on iOS.
          run.postMain(epoch) {
            sink.onCursorEvent(kind, cursorVisible, cursorX, cursorY, shapeId, hotX, hotY, scale, data)
          }
        }
        SessionSink.EV_CLOSED -> {
          run.accepting = false
          if (!run.raw) {
            run.gate.reset(waitForKeyframe = false, nowMs = SystemClock.elapsedRealtime())
            run.gate.decoderReady(false)
          }
          run.handler.post {
            RsLog.marker("SESSVIEW", mapOf("closed" to true))
            run.shutdownOnDecoderThread()
          }
          main.post {
            if (current === run) sink.onClosed(message?.toString(Charsets.UTF_8).orEmpty())
          }
        }
        SessionSink.EV_CURSOR_POS, SessionSink.EV_CURSOR_SHAPE,
        SessionSink.EV_CURSOR_USE, SessionSink.EV_CURSOR_HIDDEN -> run.postMain {
          sink.onCursorEvent(kind, cursorVisible, cursorX, cursorY, shapeId, hotX, hotY, scale, data)
        }
        SessionSink.EV_CLOCK_SYNC -> Unit
      }
    }
  }

  private fun checkMainThread() {
    check(Looper.myLooper() === Looper.getMainLooper()) { "pipeline lifecycle requires main thread" }
  }

  companion object {
    private const val GEOMETRY_TIMEOUT_MS = 100L
  }

  private fun geometry(
    srcX: Float,
    srcY: Float,
    srcW: Float,
    srcH: Float,
    displayW: Int,
    displayH: Int,
  ): StreamGeometry {
    val src = if (displayW == 0) {
      GRect(0f, 0f, 1f, 1f)
    } else {
      ViewportGeometry.source(srcX, srcY, srcW, srcH)
    }
    return StreamGeometry(src, displayW, displayH)
  }

  private data class StreamGeometry(val src: GRect, val displayW: Int, val displayH: Int)

  private inner class Run(
    val sessionId: Long, val thread: HandlerThread, val handler: Handler, val raw: Boolean = false,
  ) : SessionDecoder.Listener, RawFrameRenderer.Listener {
    // Raw mode only; created, given surfaces and released on main.
    val renderer = if (raw) RawFrameRenderer(sessionId, this) else null

    // Ingress-lock state. Nothing below executes a codec operation while holding lock.
    @Volatile var accepting = true
    @Volatile var latestGeometryEpoch: Int? = null
    val gate = SessionFrameGate<PendingFrame>().apply {
      decoderReady(false)
      reset(waitForKeyframe = true, nowMs = SystemClock.elapsedRealtime())
    }
    private val epochs = EpochTracker()
    val streamGeometry = linkedMapOf<Int, StreamGeometry>()
    var displayedGeometry = StreamGeometry(GRect(0f, 0f, 1f, 1f), 0, 0)
    var geometryAwaitingEpoch: Int? = null
    var geometryReadyEpoch: Int? = null
    var geometryTimeout = Runnable {}
    var configEpoch: Int? = null
    var configWidth = 1920
    var configHeight = 1080
    var configCodec = 0
    private var pendingEpoch: Int? = null
    private var feedScheduled = false
    private var firstFramePending = true
    var extraDropped = 0

    // Decoder-thread state; a new Run owns fresh counters and cannot affect a later start.
    var decoder: SessionDecoder? = null
    private val stats = DecodeStats()
    private var videoWidth = 0
    private var videoHeight = 0
    var videoCodec = 0
    private val startedAtMs = SystemClock.elapsedRealtime()
    private var reportAtMs = startedAtMs + 200
    private var statsAtMs = startedAtMs + 1_000
    private var logAtMs = startedAtMs + 5_000

    override fun isActive(): Boolean = current === this && accepting

    override fun isCurrent(epoch: Int): Boolean = synchronized(lock) {
      isActive() && pendingEpoch == epoch
    }

    fun observeEpochLocked(epoch: Int) {
      val change = epochs.observe(epoch)
      pendingEpoch = epoch
      if (change is EpochChange.Changed) {
        if (!raw) {
          gate.reset(waitForKeyframe = true, nowMs = SystemClock.elapsedRealtime())
          gate.decoderReady(false)
        }
        firstFramePending = true
        handler.post {
          if (isActive()) RsLog.marker("SESSVIEW", mapOf("epoch" to epoch.toUInt().toLong(), "rebuild" to true))
        }
      }
    }

    fun feedLocked() {
      if (raw) return
      if (geometryAwaitingEpoch != null && geometryReadyEpoch != geometryAwaitingEpoch) return
      if (feedScheduled) return
      feedScheduled = true
      handler.post {
        synchronized(lock) { feedScheduled = false }
        if (isActive()) decoder?.pump()
      }
    }

    fun postActionsLocked(actions: List<GateAction>) {
      // Drop is counted by gate, including reset() discards. Only JNI requests need dispatch.
      val requests = actions.filterIsInstance<GateAction.RequestKeyframe>()
      if (requests.isNotEmpty()) handler.post {
        requests.forEach { if (isActive()) RsSessionNative.requestKeyframe(sessionId, it.reason) }
      }
    }

    override fun nextFrame(): DecodeFrame? = synchronized(lock) {
      if (!isActive()) return null
      val (meta, payload) = gate.next() ?: return null
      videoCodec = payload.codec
      DecodeFrame(meta, payload, configWidth, configHeight)
    }

    override fun decoderReady(epoch: Int?, ready: Boolean) {
      synchronized(lock) {
        if (isActive() && (epoch == null || pendingEpoch == epoch)) gate.decoderReady(ready)
      }
    }

    override fun decodeFailed(epoch: Int?, droppedCurrent: Boolean) {
      synchronized(lock) {
        if (!isActive() || (epoch != null && pendingEpoch != epoch)) return
        if (droppedCurrent) extraDropped++
        postActionsLocked(gate.decodeFailed(SystemClock.elapsedRealtime()))
        gate.decoderReady(false)
      }
    }

    override fun resetForSurface(requestKeyframe: Boolean) {
      synchronized(lock) {
        if (!isActive()) return
        gate.reset(waitForKeyframe = true, nowMs = SystemClock.elapsedRealtime())
        gate.decoderReady(false)
        if (requestKeyframe) postActionsLocked(gate.decodeFailed(SystemClock.elapsedRealtime()))
      }
    }

    override fun onVideoSize(epoch: Int, width: Int, height: Int) {
      if (!isCurrent(epoch)) return
      videoWidth = width
      videoHeight = height
      postMain(epoch) { sink.onVideoSize(width, height) }
    }

    override fun onDecoded(epoch: Int, width: Int, height: Int, decodeUs: Long, latencyUs: Long?) {
      val first = synchronized(lock) {
        if (!isActive() || pendingEpoch != epoch) return
        displayedGeometry = geometryFor(epoch)
        firstFramePending.also { firstFramePending = false }
      }
      val nowMs = SystemClock.elapsedRealtime()
      stats.recordDecoded(decodeUs, latencyUs, nowMs)
      if (videoWidth != width || videoHeight != height) onVideoSize(epoch, width, height)
      postMain(epoch) { (sink as? GeometrySink)?.onVideoFrameRendered(epoch) }
      if (first) {
        RsLog.marker("SESSVIEW-FIRSTFRAME", mapOf(
          "epoch" to epoch.toUInt().toLong(), "width" to width, "height" to height,
          "ms" to (nowMs - startedAtMs),
        ))
        postMain(epoch) { sink.onFirstFrame(epoch, width, height) }
      }
    }

    /** rs-raw: replays the decode path's per-frame bookkeeping on rs-decoder. */
    override fun onRawPresented(width: Int, height: Int, copyUs: Long) {
      val epoch = synchronized(lock) { configEpoch.takeIf { isActive() } } ?: return
      handler.post { onDecoded(epoch, width, height, copyUs, null) }
    }

    private fun geometryFor(epoch: Int): StreamGeometry =
      streamGeometry[epoch] ?: StreamGeometry(GRect(0f, 0f, 1f, 1f), 0, 0)

    val timer = object : Runnable {
      override fun run() {
        if (!isActive()) return
        val nowMs = SystemClock.elapsedRealtime()
        if (!raw) {
          synchronized(lock) {
            if (!isActive()) return
            postActionsLocked(gate.tick(nowMs))
          }
        }
        if (!raw && nowMs >= reportAtMs) {
          reportAtMs = nowMs + 200
          val pending = synchronized(lock) { gate.pending }
          // R3B requests 200 ms snapshots, including the clock-synced capture latency.
          // rs_mobile.h currently documents per-frame reports and received-to-display latency.
          if (isActive()) RsSessionNative.reportDecoded(sessionId, stats.lastDecodeUs(), stats.lastLatencyUs(), pending)
        }
        if (nowMs >= statsAtMs) {
          statsAtMs = nowMs + 1_000
          val value = snapshot(nowMs)
          postMain { sink.onVideoStats(value) }
        }
        if (nowMs >= logAtMs) {
          logAtMs = nowMs + 5_000
          val value = snapshot(nowMs)
          RsLog.marker("SESSVIEW-STATS", mapOf(
            "fps" to value.fps, "decodeP50Ms" to value.decodeP50Ms, "decodeP95Ms" to value.decodeP95Ms,
            "latencyP50Ms" to value.latencyP50Ms, "queue" to value.queue, "dropped" to value.dropped,
            "keyframeRequests" to value.keyframeRequests, "width" to value.width,
            "height" to value.height, "codec" to value.codec,
          ))
        }
        if (isActive()) handler.postDelayed(this, 100)
      }
    }

    private fun snapshot(nowMs: Long): VideoStatsSnapshot {
      if (raw) {
        return stats.snapshot(nowMs, 0, 0, 0, videoWidth, videoHeight, videoCodec).copy(codec = "raw")
      }
      val counters = synchronized(lock) {
        Triple(gate.pending, gate.dropped + extraDropped, gate.keyframeRequests)
      }
      return stats.snapshot(
        nowMs, counters.first, counters.second, counters.third,
        videoWidth, videoHeight, videoCodec,
      )
    }

    fun postMain(epoch: Int? = null, block: () -> Unit) {
      main.post {
        if (isActive() && (epoch == null || isCurrent(epoch))) block()
      }
    }

    fun shutdown() {
      handler.removeCallbacks(timer)
      handler.post { shutdownOnDecoderThread() }
    }

    fun shutdownOnDecoderThread() {
      handler.removeCallbacksAndMessages(null)
      decoder?.release()
      decoder = null
      thread.quitSafely()
    }
  }
}
