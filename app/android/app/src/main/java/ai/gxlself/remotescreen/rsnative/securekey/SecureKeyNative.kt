package ai.gxlself.remotescreen.rsnative.securekey

import ai.gxlself.remotescreen.rsnative.jni.RsNative

object SecureKeyNative {
  init {
    RsNative.ensureLoaded()
  }

  fun register(spki: ByteArray): Boolean = nativeRegister(spki)

  @JvmStatic private external fun nativeRegister(spki: ByteArray): Boolean
}
