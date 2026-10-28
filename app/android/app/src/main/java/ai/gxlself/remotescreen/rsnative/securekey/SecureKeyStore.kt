package ai.gxlself.remotescreen.rsnative.securekey

import android.app.Application
import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo as AndroidKeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import ai.gxlself.remotescreen.logic.securekey.Rotation
import ai.gxlself.remotescreen.logic.securekey.RotationAction
import ai.gxlself.remotescreen.logic.securekey.Spki
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.Properties

data class KeyInfo(
  val spki: ByteArray,
  val hardwareBacked: Boolean,
  val level: String,
  val rotatedThisLaunch: Boolean,
)

data class SignResult(val signature: ByteArray, val durationMs: Double)

/** AndroidKeyStore owner for the process-wide P-256 device key. */
object SecureKeyStore {
  const val ALIAS = "rs.device.p256.v1"
  private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
  private const val MARKER_FILE = "rs.securekey"
  private const val MARKER_KEY = "installed.v1"
  private const val SAMPLE_LIMIT = 256

  private val lock = Any()
  private val samples = DoubleArray(SAMPLE_LIMIT)
  private var sampleCount = 0
  private var sampleIndex = 0
  private var application: Application? = null
  private var privateKey: PrivateKey? = null
  private var cachedInfo: KeyInfo? = null

  fun loadOrCreate(app: Application): KeyInfo = synchronized(lock) {
    application = app
    loadOrCreateLocked(app)
  }

  /** Uses the Application supplied by the launch registration. */
  fun loadOrCreate(): KeyInfo = synchronized(lock) {
    val app = application ?: throw IllegalStateException("SecureKeyStore is not initialized")
    loadOrCreateLocked(app)
  }

  fun sign(message: ByteArray): ByteArray = signWithDuration(message).signature

  fun signWithDuration(message: ByteArray): SignResult = synchronized(lock) {
    val started = System.nanoTime()
    try {
      val key = privateKey ?: run {
        val app = application ?: throw IllegalStateException("SecureKeyStore is not initialized")
        loadOrCreateLocked(app)
        privateKey ?: throw IllegalStateException("AndroidKeyStore private key is unavailable")
      }
      val signer = Signature.getInstance("SHA256withECDSA")
      signer.initSign(key)
      signer.update(message)
      SignResult(signer.sign(), elapsedMs(started))
    } finally {
      recordSample(elapsedMs(started))
    }
  }

  fun recentSignSamplesMs(): List<Double> = synchronized(lock) {
    if (sampleCount == 0) return@synchronized emptyList()
    if (sampleCount < SAMPLE_LIMIT) {
      return@synchronized samples.copyOf(sampleCount).toList()
    }
    buildList(SAMPLE_LIMIT) {
      for (offset in 0 until SAMPLE_LIMIT) {
        add(samples[(sampleIndex + offset) % SAMPLE_LIMIT])
      }
    }
  }

  private fun loadOrCreateLocked(app: Application): KeyInfo {
    val existingInfo = cachedInfo
    val existingPrivateKey = privateKey
    if (existingInfo != null && existingPrivateKey != null) return existingInfo

    val keyStore = keyStore()
    val installed = markerInstalled(app)
    val keyExists = keyStore.containsAlias(ALIAS)
    val action = Rotation.decide(installed, keyExists)
    val rotated: Boolean
    val signingKey: PrivateKey
    when (action) {
      RotationAction.USE_EXISTING -> {
        signingKey = readPrivateKey(keyStore)
        rotated = false
      }
      RotationAction.CREATE -> {
        signingKey = createKey()
        rotated = true
      }
      RotationAction.DELETE_AND_CREATE -> {
        keyStore.deleteEntry(ALIAS)
        signingKey = createKey()
        rotated = true
      }
    }

    val info = makeInfo(keyStore, signingKey, rotated)
    privateKey = signingKey
    cachedInfo = info
    markerInstalled(app, true)
    if (rotated && !info.hardwareBacked) {
      RsLog.marker("SECUREKEY", mapOf("warn" to "no-hardware-key"))
    }
    return info
  }

  private fun keyStore(): KeyStore = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }

  private fun readPrivateKey(keyStore: KeyStore): PrivateKey {
    return keyStore.getKey(ALIAS, null) as? PrivateKey
      ?: throw IllegalStateException("AndroidKeyStore key is not a private key")
  }

  private fun createKey(): PrivateKey {
    val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, KEYSTORE_PROVIDER)
    val base = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
      .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
      .setDigests(KeyProperties.DIGEST_SHA256)
      .setUserAuthenticationRequired(false)

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
      base.setIsStrongBoxBacked(true)
      try {
        return generator.generateKeyPairAfter(base)
      } catch (_: StrongBoxUnavailableException) {
        // Retry with the same policy in the TEE when StrongBox is absent.
      }
    }
    return generator.generateKeyPairAfter(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
        .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
        .setDigests(KeyProperties.DIGEST_SHA256)
        .setUserAuthenticationRequired(false),
    )
  }

  private fun KeyPairGenerator.generateKeyPairAfter(
    builder: KeyGenParameterSpec.Builder,
  ): PrivateKey {
    initialize(builder.build())
    return generateKeyPair().private
  }

  private fun makeInfo(keyStore: KeyStore, key: PrivateKey, rotated: Boolean): KeyInfo {
    val certificate = keyStore.getCertificate(ALIAS)
      ?: throw IllegalStateException("AndroidKeyStore public certificate is unavailable")
    val encoded = certificate.publicKey.encoded
    val spki = if (Spki.isP256Spki(encoded)) {
      encoded.copyOf()
    } else {
      val ec = certificate.publicKey as? ECPublicKey
        ?: throw IllegalStateException("AndroidKeyStore public key is not EC")
      Spki.fromX963(Spki.x963FromAffine(ec.w.affineX, ec.w.affineY))
    }
    val level = securityLevel(key)
    return KeyInfo(
      spki = spki,
      hardwareBacked = level == "strongbox" || level == "tee",
      level = level,
      rotatedThisLaunch = rotated,
    )
  }

  private fun securityLevel(key: PrivateKey): String {
    val factory = KeyFactory.getInstance(key.algorithm, KEYSTORE_PROVIDER)
    val info = factory.getKeySpec(key, AndroidKeyInfo::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      return when (info.securityLevel) {
        KeyProperties.SECURITY_LEVEL_STRONGBOX -> "strongbox"
        KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "tee"
        KeyProperties.SECURITY_LEVEL_SOFTWARE -> "software"
        else -> "unknown"
      }
    }
    return if (info.isInsideSecureHardware) "tee" else "software"
  }

  private fun markerFile(app: Application): File = File(app.noBackupFilesDir, MARKER_FILE)

  private fun markerInstalled(app: Application): Boolean {
    val file = markerFile(app)
    if (file.isFile) {
      val properties = Properties()
      try {
        FileInputStream(file).use { properties.load(it) }
        return properties.getProperty(MARKER_KEY) == "true"
      } catch (_: Exception) {
        // Fall through to the SharedPreferences copy below.
      }
    }
    return app.getSharedPreferences(MARKER_FILE, Context.MODE_PRIVATE)
      .getBoolean(MARKER_KEY, false)
  }

  private fun markerInstalled(app: Application, installed: Boolean) {
    // Keep the named SharedPreferences entry for the Android preference API;
    // the no-backup copy is authoritative across backup/restore operations.
    check(app.getSharedPreferences(MARKER_FILE, Context.MODE_PRIVATE)
      .edit()
      .putBoolean(MARKER_KEY, installed)
      .commit()) { "cannot persist secure key SharedPreferences marker" }
    val file = markerFile(app)
    val temporary = File(file.parentFile, "$MARKER_FILE.tmp")
    val properties = Properties()
    properties[MARKER_KEY] = installed.toString()
    try {
      FileOutputStream(temporary).use { properties.store(it, null) }
      if (!temporary.renameTo(file)) {
        temporary.delete()
        throw IllegalStateException("cannot replace secure key marker")
      }
    } catch (error: Exception) {
      temporary.delete()
      throw IllegalStateException("cannot persist secure key marker", error)
    }
  }

  private fun elapsedMs(started: Long): Double =
    (System.nanoTime() - started).toDouble() / 1_000_000.0

  private fun recordSample(durationMs: Double) {
    samples[sampleIndex] = durationMs
    sampleIndex = (sampleIndex + 1) % SAMPLE_LIMIT
    sampleCount = minOf(sampleCount + 1, SAMPLE_LIMIT)
  }
}
