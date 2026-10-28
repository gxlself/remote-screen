package ai.gxlself.remotescreen.rsnative.recording

import ai.gxlself.remotescreen.logic.recording.WrapCall
import ai.gxlself.remotescreen.rsnative.jni.RsNative

object RecordingKeyNative {
  init {
    RsNative.ensureLoaded()
  }

  fun register(): Boolean = nativeRegister()

  @JvmStatic
  private external fun nativeRegister(): Boolean

  /** JNI callback entry point; no exception may escape to native code. */
  @JvmStatic
  fun wrapFromNative(op: Int, input: ByteArray): ByteArray? {
    return try {
      when (
        val outcome = WrapCall.handle(
          op,
          input,
          RecordingKeyStore::encrypt,
          RecordingKeyStore::decrypt,
        )
      ) {
        is WrapCall.Outcome.Ok -> outcome.bytes
        WrapCall.Outcome.Unavailable -> null
        WrapCall.Outcome.Rejected -> ByteArray(0)
      }
    } catch (_: Throwable) {
      null
    }
  }
}
