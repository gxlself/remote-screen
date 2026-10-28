package ai.gxlself.remotescreen.rsnative.audio

import ai.gxlself.remotescreen.specs.NativeRsAudioSpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext

/** TurboModule implementation for remote audio playback. */
class RsAudioModule(
  reactContext: ReactApplicationContext,
) : NativeRsAudioSpec(reactContext) {
  private val emitterId: Int = RsAudioController.registerEmitter(reactContext) { event ->
    val value = Arguments.createMap()
    value.putString("state", event.state)
    value.putString("reason", event.reason)
    emitOnAudioState(value)
  }

  override fun start(sessionId: String, promise: Promise) {
    RsAudioController.start(sessionId) { result ->
      promise.resolve(result)
    }
  }

  override fun stop() {
    RsAudioController.stop()
  }

  override fun setMuted(muted: Boolean) {
    RsAudioController.setMuted(muted)
  }

  override fun getStats(promise: Promise) {
    RsAudioController.stats { stats ->
      val value = Arguments.createMap()
      value.putBoolean("running", stats.running)
      value.putDouble("packets", stats.packets.toDouble())
      value.putDouble("lost", stats.lost.toDouble())
      value.putDouble("concealed", stats.concealed.toDouble())
      value.putDouble("underruns", stats.underruns.toDouble())
      value.putDouble("targetMs", stats.targetMs.toDouble())
      value.putDouble("levelMs", stats.levelMs.toDouble())
      value.putDouble("outputLatencyMs", stats.outputLatencyMs)
      value.putDouble("ioBufferMs", stats.ioBufferMs)
      value.putString("route", stats.route)
      promise.resolve(value)
    }
  }

  override fun invalidate() {
    RsAudioController.unregisterEmitter(emitterId)
    RsAudioController.stop()
    super.invalidate()
  }

  companion object {
    const val NAME = "RsAudio"
  }
}
