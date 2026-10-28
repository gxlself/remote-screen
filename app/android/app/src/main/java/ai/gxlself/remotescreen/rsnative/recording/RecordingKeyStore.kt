package ai.gxlself.remotescreen.rsnative.recording

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo as AndroidKeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import ai.gxlself.remotescreen.logic.recording.KeyBlob
import ai.gxlself.remotescreen.logic.recording.WrapCall
import java.security.KeyFactory
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Android Keystore owner for the process-wide recording wrapping key. */
object RecordingKeyStore {
  const val ALIAS = "rs.recording.master.v1"

  private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
  private const val TRANSFORMATION = "AES/GCM/NoPadding"
  private val lock = Any()

  @Volatile
  private var cachedKey: SecretKey? = null

  fun encrypt(plain: ByteArray): Pair<ByteArray, ByteArray> {
    require(plain.size == KeyBlob.KEY_LEN) { "recording key must be ${KeyBlob.KEY_LEN} bytes" }
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, key())
    cipher.updateAAD(KeyBlob.AAD)
    return cipher.iv.copyOf() to cipher.doFinal(plain)
  }

  fun decrypt(iv: ByteArray, ctWithTag: ByteArray): ByteArray {
    require(iv.size == KeyBlob.IV_LEN) { "recording key IV must be ${KeyBlob.IV_LEN} bytes" }
    require(ctWithTag.size == KeyBlob.KEY_LEN + KeyBlob.TAG_LEN) {
      "recording key ciphertext must be ${KeyBlob.KEY_LEN + KeyBlob.TAG_LEN} bytes"
    }
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
    cipher.updateAAD(KeyBlob.AAD)
    return cipher.doFinal(ctWithTag)
  }

  /** Returns whether the Keystore key is backed by StrongBox, when available. */
  fun isStrongBox(): Boolean {
    return try {
      val info = keyInfo(key())
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
        info.securityLevel == KeyProperties.SECURITY_LEVEL_STRONGBOX
    } catch (_: Exception) {
      false
    }
  }

  private fun key(): SecretKey {
    cachedKey?.let { return it }
    return synchronized(lock) {
      cachedKey?.let { return@synchronized it }
      val loaded = try {
        val store = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        store.getKey(ALIAS, null)
      } catch (error: Exception) {
        throw WrapCall.KeyUnavailable(error)
      }
      val result = when {
        loaded == null -> try {
          createKey()
        } catch (error: WrapCall.KeyUnavailable) {
          throw error
        } catch (error: Exception) {
          throw WrapCall.KeyUnavailable(error)
        }

        loaded is SecretKey -> loaded
        else -> throw WrapCall.KeyUnavailable(IllegalStateException("AndroidKeyStore key is not AES"))
      }
      cachedKey = result
      result
    }
  }

  private fun createKey(): SecretKey {
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
      try {
        return generate(generator, strongBox = true)
      } catch (_: StrongBoxUnavailableException) {
        // Retry with the same key policy in the TEE when StrongBox is absent.
      }
    }
    return generate(generator, strongBox = false)
  }

  private fun generate(generator: KeyGenerator, strongBox: Boolean): SecretKey {
    val builder = KeyGenParameterSpec.Builder(
      ALIAS,
      KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
    )
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setKeySize(256)
      .setRandomizedEncryptionRequired(true)
      .setUserAuthenticationRequired(false)
    if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
      builder.setIsStrongBoxBacked(true)
    }
    generator.init(builder.build())
    return generator.generateKey()
  }

  private fun keyInfo(key: SecretKey): AndroidKeyInfo {
    val factory = KeyFactory.getInstance(key.algorithm, KEYSTORE_PROVIDER)
    return factory.getKeySpec(key, AndroidKeyInfo::class.java)
  }
}
