package ai.gxlself.remotescreen.rsnative.securekey

import android.app.Application
import ai.gxlself.remotescreen.logic.securekey.SignStats
import ai.gxlself.remotescreen.logic.securekey.Spki
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.security.SecureRandom

data class SecureKeyRegistrationSnapshot(
  val registered: Boolean,
  val secureEnclave: Boolean,
  val fp: String,
  val deviceId: String,
  val rotatedThisLaunch: Boolean,
  val level: String,
  val error: String?,
)

data class SigningBenchmark(
  val count: Int,
  val p50Ms: Double,
  val p95Ms: Double,
  val maxMs: Double,
)

object SecureKeySigner {
  private val stateLock = Any()
  private val random = SecureRandom()
  private var registration: SecureKeyRegistrationSnapshot? = null
  private var lastSignErrorLogMs = Long.MIN_VALUE

  @JvmStatic
  fun signFromNative(message: ByteArray): ByteArray? {
    return try {
      SecureKeyStore.sign(message)
    } catch (error: Throwable) {
      logSignError(error)
      null
    }
  }

  fun registerAtLaunch(app: Application) {
    try {
      val info = SecureKeyStore.loadOrCreate(app)
      val fingerprint = Spki.fingerprint(info.spki)
      val snapshot = SecureKeyRegistrationSnapshot(
        registered = SecureKeyNative.register(info.spki),
        secureEnclave = info.hardwareBacked,
        fp = Spki.formatFingerprint(fingerprint),
        deviceId = Spki.deviceId(fingerprint),
        rotatedThisLaunch = info.rotatedThisLaunch,
        level = info.level,
        error = null,
      )
      if (!snapshot.registered) {
        throw IllegalStateException("native signer registration failed")
      }
      synchronized(stateLock) {
        registration = snapshot
      }
      RsLog.marker(
        "SECUREKEY",
        mapOf(
          "registered" to true,
          "fp" to snapshot.fp,
          "deviceId" to snapshot.deviceId,
          "rotated" to snapshot.rotatedThisLaunch,
          "level" to snapshot.level,
        ),
      )
    } catch (error: Throwable) {
      val snapshot = SecureKeyRegistrationSnapshot(
        registered = false,
        secureEnclave = false,
        fp = "",
        deviceId = "",
        rotatedThisLaunch = false,
        level = "",
        error = error.message ?: error::class.java.simpleName,
      )
      synchronized(stateLock) {
        registration = snapshot
      }
      RsLog.marker("SECUREKEY", mapOf("registered" to false, "error" to (snapshot.error ?: "error")))
    }
  }

  fun snapshot(): SecureKeyRegistrationSnapshot = synchronized(stateLock) {
    registration ?: SecureKeyRegistrationSnapshot(
      registered = false,
      secureEnclave = false,
      fp = "",
      deviceId = "",
      rotatedThisLaunch = false,
      level = "",
      error = null,
    )
  }

  fun benchmark(count: Int): SigningBenchmark {
    val boundedCount = count.coerceIn(1, 500)
    val samples = ArrayList<Double>(boundedCount)
    repeat(boundedCount) {
      val message = ByteArray(random.nextInt(481) + 32)
      random.nextBytes(message)
      samples += SecureKeyStore.signWithDuration(message).durationMs
    }
    val summary = SignStats.summary(samples)
    return SigningBenchmark(
      count = boundedCount,
      p50Ms = SignStats.round2(summary.p50),
      p95Ms = SignStats.round2(summary.p95),
      maxMs = SignStats.round2(summary.max),
    )
  }

  private fun logSignError(error: Throwable) {
    val nowMs = System.nanoTime() / 1_000_000L
    synchronized(stateLock) {
      if (nowMs - lastSignErrorLogMs < 60_000L) return
      lastSignErrorLogMs = nowMs
    }
    RsLog.marker(
      "SECUREKEY",
      mapOf("sign" to "error", "class" to error::class.java.simpleName),
    )
  }
}
