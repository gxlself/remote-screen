package ai.gxlself.remotescreen.logic.adapters

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.util.logging.Logger

object AdapterSecretFormat {
  const val DIR = "adapter-secrets"
  const val MAX_KEY_BYTES = 300
  const val MAX_VALUE_BYTES = 4096

  private val aadPrefix = "rs.adapter.secret.v1".toByteArray(StandardCharsets.UTF_8)
  private val hex = "0123456789abcdef".toCharArray()

  /** A key is a non-empty, strict UTF-8 byte sequence without NUL. */
  fun validKey(key: ByteArray): Boolean {
    if (key.isEmpty() || key.size > MAX_KEY_BYTES || key.any { it == 0.toByte() }) {
      return false
    }
    return try {
      val decoder = StandardCharsets.UTF_8.newDecoder()
        .onMalformedInput(CodingErrorAction.REPORT)
        .onUnmappableCharacter(CodingErrorAction.REPORT)
      decoder.decode(ByteBuffer.wrap(key))
      true
    } catch (_: CharacterCodingException) {
      false
    }
  }

  /** SHA-256(key), rendered as lower-case hexadecimal and suffixed with .bin. */
  fun fileName(key: ByteArray): String {
    val digest = MessageDigest.getInstance("SHA-256").digest(key)
    val result = CharArray(digest.size * 2 + 4)
    digest.forEachIndexed { index, byte ->
      val value = byte.toInt() and 0xff
      result[index * 2] = hex[value ushr 4]
      result[index * 2 + 1] = hex[value and 0x0f]
    }
    result[result.size - 4] = '.'
    result[result.size - 3] = 'b'
    result[result.size - 2] = 'i'
    result[result.size - 1] = 'n'
    return String(result)
  }

  /** Binds an encrypted value to both the protocol and its logical key. */
  fun aad(key: ByteArray): ByteArray =
    ByteArray(aadPrefix.size + 1 + key.size).also {
      aadPrefix.copyInto(it)
      it[aadPrefix.size] = 0
      key.copyInto(it, aadPrefix.size + 1)
    }

  /** Encodes version, 12-byte GCM IV, ciphertext and its 16-byte tag. */
  fun pack(iv: ByteArray, ct: ByteArray): ByteArray {
    require(iv.size == 12) { "invalid IV" }
    require(ct.size >= 16) { "invalid ciphertext" }
    return ByteArray(1 + iv.size + ct.size).also {
      it[0] = 1
      iv.copyInto(it, 1)
      ct.copyInto(it, 1 + iv.size)
    }
  }

  /** Decodes a versioned blob, or null when its framing is invalid. */
  fun unpack(blob: ByteArray): Pair<ByteArray, ByteArray>? {
    if (blob.size < 1 + 12 + 16 || blob[0].toInt() != 1) return null
    return blob.copyOfRange(1, 13) to blob.copyOfRange(13, blob.size)
  }
}

/** Platform cipher callbacks. Authentication failures are represented by null. */
class AdapterSecretCipher(
  val encrypt: (plain: ByteArray, aad: ByteArray) -> Pair<ByteArray, ByteArray>,
  val decrypt: (iv: ByteArray, ct: ByteArray, aad: ByteArray) -> ByteArray?,
)

class AdapterSecretFileStore(
  private val dir: File,
  private val cipher: AdapterSecretCipher,
) {
  sealed interface Get {
    class Found(val value: ByteArray) : Get
    object Missing : Get
    object Failed : Get
  }

  private val logger = Logger.getLogger(AdapterSecretFileStore::class.java.name)

  @Synchronized
  fun get(key: ByteArray): Get {
    if (!AdapterSecretFormat.validKey(key)) return Get.Failed
    val file = File(dir, AdapterSecretFormat.fileName(key))
    if (!file.exists()) return Get.Missing

    val blob = try {
      file.readBytes()
    } catch (_: IOException) {
      return Get.Failed
    }
    val unpacked = AdapterSecretFormat.unpack(blob)
    if (unpacked == null) {
      discardCorrupt(file)
      return Get.Missing
    }

    val (iv, ct) = unpacked
    var plain: ByteArray? = null
    return try {
      plain = cipher.decrypt(iv, ct, AdapterSecretFormat.aad(key))
      if (plain == null) {
        discardCorrupt(file)
        Get.Missing
      } else if (plain.size > AdapterSecretFormat.MAX_VALUE_BYTES) {
        Get.Failed
      } else {
        val result = Get.Found(plain.copyOf())
        Get.Found(result.value)
      }
    } catch (_: Exception) {
      Get.Failed
    } finally {
      plain?.fill(0)
    }
  }

  @Synchronized
  fun put(key: ByteArray, value: ByteArray): Boolean {
    if (!AdapterSecretFormat.validKey(key) ||
      value.isEmpty() || value.size > AdapterSecretFormat.MAX_VALUE_BYTES
    ) {
      return false
    }

    if ((!dir.exists() && !dir.mkdirs()) || !dir.isDirectory) return false
    val name = AdapterSecretFormat.fileName(key)
    val destination = File(dir, name)
    val temporary = File(dir, "$name.tmp")
    var plain = value.copyOf()
    try {
      val encrypted = cipher.encrypt(plain, AdapterSecretFormat.aad(key))
      val blob = AdapterSecretFormat.pack(encrypted.first, encrypted.second)
      FileOutputStream(temporary).use { output ->
        output.write(blob)
        output.flush()
        output.fd.sync()
      }
      if (!temporary.renameTo(destination)) return false
      return true
    } catch (_: Exception) {
      return false
    } finally {
      plain.fill(0)
      if (temporary.exists()) temporary.delete()
    }
  }

  @Synchronized
  fun delete(key: ByteArray): Boolean {
    if (!AdapterSecretFormat.validKey(key)) return false
    val file = File(dir, AdapterSecretFormat.fileName(key))
    return !file.exists() || file.delete()
  }

  private fun discardCorrupt(file: File) {
    file.delete()
    logger.warning("adapter secret discarded")
  }
}
