package ai.gxlself.remotescreen.rsnative.adapters

import android.app.Application
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import ai.gxlself.remotescreen.logic.adapters.AdapterSecretCipher
import ai.gxlself.remotescreen.logic.adapters.AdapterSecretFileStore
import ai.gxlself.remotescreen.logic.adapters.AdapterSecretFormat
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsAdapterSecretNative
import java.io.File
import java.security.KeyStore
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object AdapterSecretStore {
  const val ALIAS = "rs.adapter.secrets.v1"

  private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
  private val stateLock = Any()
  private val executor: ExecutorService = Executors.newSingleThreadExecutor { runnable ->
    Thread(runnable, "rs-adapter-secrets").apply { isDaemon = true }
  }
  @Volatile private var store: AdapterSecretFileStore? = null
  private var started = false

  /** Prepares the file store off the launch thread and registers its callbacks once. */
  fun start(app: Application) {
    synchronized(stateLock) {
      if (started) return
      started = true
      val directory = File(app.filesDir, AdapterSecretFormat.DIR)
      executor.execute {
        val registered = try {
          if ((!directory.exists() && !directory.mkdirs()) || !directory.isDirectory) {
            throw IllegalStateException("adapter secret directory unavailable")
          }
          store = AdapterSecretFileStore(directory, makeCipher())
          RsAdapterSecretNative.register()
        } catch (_: Throwable) {
          false
        }
        RsLog.marker("ADAPTER-SECRETS", mapOf("registered" to registered))
      }
    }
  }

  @JvmStatic
  fun getFromNative(key: ByteArray): ByteArray? {
    val fileStore = store ?: throw IllegalStateException("adapter secret store unavailable")
    return when (val result = fileStore.get(key)) {
      is AdapterSecretFileStore.Get.Found -> result.value
      AdapterSecretFileStore.Get.Missing -> null
      AdapterSecretFileStore.Get.Failed -> throw IllegalStateException("adapter secret read failed")
    }
  }

  @JvmStatic
  fun putFromNative(key: ByteArray, value: ByteArray): Boolean {
    return try {
      store?.put(key, value) ?: false
    } finally {
      value.fill(0)
    }
  }

  @JvmStatic
  fun deleteFromNative(key: ByteArray): Boolean = store?.delete(key) ?: false

  private fun makeCipher(): AdapterSecretCipher = AdapterSecretCipher(
    encrypt = { plain, aad ->
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.ENCRYPT_MODE, key())
      cipher.updateAAD(aad)
      cipher.iv.copyOf() to cipher.doFinal(plain)
    },
    decrypt = { iv, ciphertext, aad ->
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
      cipher.updateAAD(aad)
      try {
        cipher.doFinal(ciphertext)
      } catch (_: AEADBadTagException) {
        null
      }
    },
  )

  private fun key(): SecretKey {
    val keyStore = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
    val existing = keyStore.getKey(ALIAS, null)
    if (existing != null) return existing as? SecretKey
      ?: throw IllegalStateException("AndroidKeyStore secret key has wrong type")

    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
    generator.init(
      KeyGenParameterSpec.Builder(
        ALIAS,
        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
      )
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .setRandomizedEncryptionRequired(true)
        .setUserAuthenticationRequired(false)
        .build(),
    )
    return generator.generateKey()
  }
}
