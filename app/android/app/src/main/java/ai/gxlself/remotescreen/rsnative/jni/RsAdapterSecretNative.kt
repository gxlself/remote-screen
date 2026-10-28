package ai.gxlself.remotescreen.rsnative.jni

object RsAdapterSecretNative {
  init {
    RsNative.ensureLoaded()
  }

  fun register(): Boolean = nativeRegister()

  @JvmStatic private external fun nativeRegister(): Boolean
}
