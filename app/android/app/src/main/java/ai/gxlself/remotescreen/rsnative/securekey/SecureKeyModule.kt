package ai.gxlself.remotescreen.rsnative.securekey

import ai.gxlself.remotescreen.specs.NativeSecureKeySpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class SecureKeyModule(reactContext: ReactApplicationContext) : NativeSecureKeySpec(reactContext) {
  override fun getName(): String = NAME

  override fun info(promise: Promise) {
    val snapshot = SecureKeySigner.snapshot()
    promise.resolve(
      Arguments.createMap().apply {
        putBoolean("registered", snapshot.registered)
        putBoolean("secureEnclave", snapshot.secureEnclave)
        putString("fp", if (snapshot.registered) snapshot.fp else "")
        putString("deviceId", if (snapshot.registered) snapshot.deviceId else "")
        putBoolean("rotatedThisLaunch", snapshot.registered && snapshot.rotatedThisLaunch)
      },
    )
  }

  override fun benchmark(count: Double, promise: Promise) {
    WORKER.execute {
      try {
        val result = SecureKeySigner.benchmark(count.toInt())
        promise.resolve(
          Arguments.createMap().apply {
            putInt("count", result.count)
            putDouble("p50Ms", result.p50Ms)
            putDouble("p95Ms", result.p95Ms)
            putDouble("maxMs", result.maxMs)
          },
        )
      } catch (error: Throwable) {
        promise.reject(
          "E_SECUREKEY",
          error.message ?: "device key signing failed",
          error,
        )
      }
    }
  }

  companion object {
    const val NAME = "SecureKey"
    private val WORKER: ExecutorService = Executors.newSingleThreadExecutor { runnable ->
      Thread(runnable, "rs-secure-key-module").apply { isDaemon = true }
    }
  }
}
