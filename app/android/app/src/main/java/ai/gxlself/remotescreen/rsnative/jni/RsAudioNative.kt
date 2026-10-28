package ai.gxlself.remotescreen.rsnative.jni

/** JNI bridge for the process-wide AAudio output stream. */
object RsAudioNative {
  init {
    RsNative.ensureLoaded()
  }

  @JvmStatic external fun nativeOpen(sessionId: Long): Boolean

  @JvmStatic external fun nativeClose()

  @JvmStatic external fun nativeStartStream(): Int

  @JvmStatic external fun nativeStopStream()

  @JvmStatic external fun nativeSetMuted(muted: Boolean)

  @JvmStatic external fun nativeStats(out: LongArray): Boolean
}
