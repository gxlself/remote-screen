package ai.gxlself.remotescreen.rsnative.bootstrap

import android.app.Application
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.adapters.AdapterSecretStore
import ai.gxlself.remotescreen.rsnative.crash.NativeCrashCollector
import ai.gxlself.remotescreen.rsnative.jni.RsNative
import ai.gxlself.remotescreen.rsnative.lifecycle.LifecycleBootstrap
import ai.gxlself.remotescreen.rsnative.securekey.SecureKeyBootstrap
import ai.gxlself.remotescreen.rsnative.recording.RecordingKeyBootstrap

object RsBootstrap {
  private val expectedAbi = intArrayOf(104, 136, 64)

  fun start(app: Application) {
    try {
      NativeCrashCollector.start(app)
    } catch (error: Throwable) {
      RsLog.marker(
        "BOOT",
        mapOf("crash" to "error", "message" to (error.message ?: error::class.java.simpleName)),
      )
    }
    try {
      RsNative.ensureLoaded()
    } catch (error: UnsatisfiedLinkError) {
      RsLog.marker(
        "NATIVE",
        mapOf("loaded" to false, "error" to (error.message ?: error::class.java.simpleName)),
      )
      return
    }

    val abi = RsNative.abiSizes()
    RsLog.marker(
      "NATIVE",
      mapOf("loaded" to true, "abi" to abi.toList(), "nowUs" to RsNative.nowUs()),
    )
    if (!abi.contentEquals(expectedAbi)) {
      RsLog.marker(
        "NATIVE",
        mapOf("abiMismatch" to true, "abi" to abi.toList(), "expected" to expectedAbi.toList()),
      )
    }

    try {
      SecureKeyBootstrap.start(app)
    } catch (error: Throwable) {
      RsLog.marker(
        "BOOT",
        mapOf("secureKey" to "error", "message" to (error.message ?: error::class.java.simpleName)),
      )
    }
    try {
      RecordingKeyBootstrap.start(app)
    } catch (error: Throwable) {
      RsLog.marker(
        "BOOT",
        mapOf("recordingKey" to "error", "message" to (error.message ?: error::class.java.simpleName)),
      )
    }
    try {
      AdapterSecretStore.start(app)
    } catch (error: Throwable) {
      RsLog.marker(
        "BOOT",
        mapOf("adapterSecrets" to "error", "message" to (error.message ?: error::class.java.simpleName)),
      )
    }
    try {
      LifecycleBootstrap.start(app)
    } catch (error: Throwable) {
      RsLog.marker(
        "BOOT",
        mapOf("lifecycle" to "error", "message" to (error.message ?: error::class.java.simpleName)),
      )
    }
  }
}
