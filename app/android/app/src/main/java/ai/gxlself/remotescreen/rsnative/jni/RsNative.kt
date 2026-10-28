package ai.gxlself.remotescreen.rsnative.jni

object RsNative {
  init {
    System.loadLibrary("react-native-rs-core")
  }

  fun ensureLoaded() {}

  fun nowUs(): Long = nativeNowUs()

  fun abiSizes(): IntArray = nativeAbiSizes()

  @JvmStatic private external fun nativeNowUs(): Long

  @JvmStatic private external fun nativeAbiSizes(): IntArray
}
