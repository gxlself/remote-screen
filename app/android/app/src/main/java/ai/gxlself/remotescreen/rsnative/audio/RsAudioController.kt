package ai.gxlself.remotescreen.rsnative.audio

import android.content.Context
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import ai.gxlself.remotescreen.logic.audio.AudioPlaybackAction
import ai.gxlself.remotescreen.logic.audio.AudioPlaybackEvent
import ai.gxlself.remotescreen.logic.audio.AudioPlaybackMachine
import ai.gxlself.remotescreen.logic.audio.AudioPlaybackState
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsAudioNative

/** Main-thread adapter that translates the pure playback machine to Android. */
object RsAudioController {
  data class Stats(
    val running: Boolean,
    val packets: Long,
    val lost: Long,
    val concealed: Long,
    val underruns: Long,
    val targetMs: Long,
    val levelMs: Long,
    val outputLatencyMs: Double,
    val ioBufferMs: Double,
    val route: String,
  )

  data class StateEvent(val state: String, val reason: String)

  private class EngineFailure(val failureReason: String) : RuntimeException()

  private val mainHandler = Handler(Looper.getMainLooper())
  private val emitterLock = Any()
  private val emitters = linkedMapOf<Int, (StateEvent) -> Unit>()
  private var nextEmitterId = 1
  private var initialized = false
  private var appContext: Context? = null
  private var audioManager: AudioManager? = null
  private var machine = AudioPlaybackMachine()
  private var muted = false

  private val lifecycleObserver = object : DefaultLifecycleObserver {
    override fun onStart(owner: LifecycleOwner) {
      process(AudioPlaybackEvent.EnterForeground)
    }

    override fun onStop(owner: LifecycleOwner) {
      process(AudioPlaybackEvent.EnterBackground)
    }
  }

  private val deviceCallback = object : AudioDeviceCallback() {
    override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) {
      process(AudioPlaybackEvent.ConfigurationChanged)
    }

    override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) {
      process(AudioPlaybackEvent.ConfigurationChanged)
    }
  }

  /** Called by each TurboModule instance before it registers its event sink. */
  fun initialize(context: Context) {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      initializeOnMain(context)
    } else {
      mainHandler.post { initializeOnMain(context) }
    }
  }

  fun registerEmitter(context: Context, emitter: (StateEvent) -> Unit): Int {
    initialize(context)
    synchronized(emitterLock) {
      val id = nextEmitterId++
      emitters[id] = emitter
      return id
    }
  }

  fun unregisterEmitter(id: Int) {
    synchronized(emitterLock) {
      emitters.remove(id)
    }
  }

  fun start(sessionId: String, completion: (Boolean) -> Unit) {
    mainHandler.post {
      val parsed = parseSessionId(sessionId)
      if (parsed == null) {
        RsLog.marker("AUDIO", mapOf("open" to "null"))
        completion(false)
        return@post
      }
      completion(process(AudioPlaybackEvent.Start(sessionId)))
    }
  }

  fun stop() {
    mainHandler.post {
      process(AudioPlaybackEvent.Stop)
    }
  }

  fun setMuted(value: Boolean) {
    mainHandler.post {
      muted = value
      RsAudioNative.nativeSetMuted(value)
    }
  }

  fun stats(completion: (Stats) -> Unit) {
    mainHandler.post {
      val values = LongArray(STATS_LENGTH)
      val available = runCatching { RsAudioNative.nativeStats(values) }.getOrDefault(false)
      if (!available) values.fill(0L)
      val frameBurst = values[FRAME_BURST_INDEX]
      val bufferFrames = values[BUFFER_SIZE_INDEX]
      completion(
        Stats(
          running = machine.state is AudioPlaybackState.Running,
          packets = values[PACKETS_INDEX],
          lost = values[LOST_INDEX],
          concealed = values[CONCEALED_INDEX],
          underruns = values[UNDERRUNS_INDEX],
          targetMs = values[TARGET_MS_INDEX],
          levelMs = values[LEVEL_MS_INDEX],
          outputLatencyMs = bufferFrames / SAMPLE_RATE_KHZ,
          ioBufferMs = frameBurst / SAMPLE_RATE_KHZ,
          route = routeName(),
        ),
      )
    }
  }

  private fun initializeOnMain(context: Context) {
    if (initialized) return
    appContext = context.applicationContext
    audioManager = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
    ProcessLifecycleOwner.get().lifecycle.addObserver(lifecycleObserver)
    audioManager?.registerAudioDeviceCallback(deviceCallback, mainHandler)
    initialized = true
  }

  private fun process(event: AudioPlaybackEvent): Boolean {
    val actions = machine.handle(event)
    return try {
      actions.forEach(::execute)
      true
    } catch (failure: EngineFailure) {
      val cleanup = machine.handle(AudioPlaybackEvent.EngineStartFailed(failure.failureReason))
      cleanup.forEach { action ->
        runCatching { execute(action) }
      }
      false
    } catch (failure: Throwable) {
      val reason = failure.message ?: failure::class.java.simpleName
      val cleanup = machine.handle(AudioPlaybackEvent.EngineStartFailed(reason))
      cleanup.forEach { action ->
        runCatching { execute(action) }
      }
      false
    }
  }

  private fun execute(action: AudioPlaybackAction) {
    when (action) {
      AudioPlaybackAction.ActivateSession ->
        RsLog.marker("AUDIO", mapOf("session" to "activate", "mixWithOthers" to true))
      AudioPlaybackAction.DeactivateSession ->
        RsLog.marker("AUDIO", mapOf("session" to "deactivate"))
      is AudioPlaybackAction.OpenAudio -> {
        val id = parseSessionId(action.session)
          ?: throw EngineFailure("open-null")
        val opened = runCatching { RsAudioNative.nativeOpen(id) }.getOrDefault(false)
        if (!opened) {
          RsLog.marker("AUDIO", mapOf("open" to "null"))
          throw EngineFailure("open-null")
        }
        RsAudioNative.nativeSetMuted(muted)
      }
      AudioPlaybackAction.CloseAudio -> RsAudioNative.nativeClose()
      AudioPlaybackAction.StartEngine -> {
        val result = runCatching { RsAudioNative.nativeStartStream() }
          .getOrElse { throw EngineFailure("aaudio:${it.message ?: it::class.java.simpleName}") }
        if (result != 0) throw EngineFailure("aaudio:$result")
      }
      AudioPlaybackAction.StopEngine -> RsAudioNative.nativeStopStream()
      AudioPlaybackAction.RebuildEngine -> {
        RsAudioNative.nativeStopStream()
        val result = RsAudioNative.nativeStartStream()
        if (result != 0 && machine.state is AudioPlaybackState.Running) {
          throw EngineFailure("aaudio:$result")
        }
      }
      is AudioPlaybackAction.Emit -> emit(action.state, action.reason)
    }
  }

  private fun emit(state: AudioPlaybackState, reason: String) {
    val route = routeName()
    val values = LongArray(STATS_LENGTH)
    val available = runCatching { RsAudioNative.nativeStats(values) }.getOrDefault(false)
    if (!available) values.fill(0L)
    RsLog.marker(
      "AUDIO",
      mapOf(
        "state" to state.name,
        "reason" to reason,
        "route" to route,
        "ioBufferMs" to values[FRAME_BURST_INDEX] / SAMPLE_RATE_KHZ,
        "outputLatencyMs" to values[BUFFER_SIZE_INDEX] / SAMPLE_RATE_KHZ,
      ),
    )
    val event = StateEvent(state.name, reason)
    val sinks = synchronized(emitterLock) { emitters.values.toList() }
    sinks.forEach { sink ->
      runCatching { sink(event) }
        .onFailure { error ->
          RsLog.marker(
            "AUDIO",
            mapOf("event" to "emit-error", "error" to (error.message ?: error::class.java.simpleName)),
          )
        }
    }
  }

  private fun routeName(): String {
    val manager = audioManager ?: return "other"
    val devices = runCatching { manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS) }
      .getOrNull()
      ?: return "other"
    val candidate = devices
      .asSequence()
      .filter { it.isSink }
      .sortedBy { routePriority(it.type) }
      .firstOrNull()
      ?: return "other"
    return when (candidate.type) {
      AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
      AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE,
      AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
      -> "speaker"
      AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
      AudioDeviceInfo.TYPE_WIRED_HEADSET,
      AudioDeviceInfo.TYPE_LINE_ANALOG,
      AudioDeviceInfo.TYPE_LINE_DIGITAL,
      -> "wired"
      AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
      AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
      AudioDeviceInfo.TYPE_BLE_HEADSET,
      AudioDeviceInfo.TYPE_HEARING_AID,
      -> "bluetooth"
      AudioDeviceInfo.TYPE_USB_ACCESSORY,
      AudioDeviceInfo.TYPE_USB_DEVICE,
      AudioDeviceInfo.TYPE_USB_HEADSET,
      -> "usb"
      else -> "other"
    }
  }

  private fun routePriority(type: Int): Int = when (type) {
    AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
    AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
    AudioDeviceInfo.TYPE_BLE_HEADSET,
    AudioDeviceInfo.TYPE_HEARING_AID,
    -> 0
    AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
    AudioDeviceInfo.TYPE_WIRED_HEADSET,
    AudioDeviceInfo.TYPE_LINE_ANALOG,
    AudioDeviceInfo.TYPE_LINE_DIGITAL,
    -> 1
    AudioDeviceInfo.TYPE_USB_ACCESSORY,
    AudioDeviceInfo.TYPE_USB_DEVICE,
    AudioDeviceInfo.TYPE_USB_HEADSET,
    -> 2
    AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
    AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE,
    AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
    -> 3
    else -> 4
  }

  private fun parseSessionId(value: String): Long? =
    runCatching { value.toULong().toLong() }.getOrNull()

  private const val STATS_LENGTH = 14
  private const val PACKETS_INDEX = 0
  private const val LOST_INDEX = 1
  private const val CONCEALED_INDEX = 2
  private const val UNDERRUNS_INDEX = 5
  private const val TARGET_MS_INDEX = 6
  private const val LEVEL_MS_INDEX = 7
  private const val FRAME_BURST_INDEX = 11
  private const val BUFFER_SIZE_INDEX = 12
  private const val SAMPLE_RATE_KHZ = 48.0
}
