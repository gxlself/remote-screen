package ai.gxlself.remotescreen.logic.recording

import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeyBlobTest {
  private val key = SecretKeySpec(ByteArray(KeyBlob.KEY_LEN) { it.toByte() }, "AES")

  @Test
  fun packAndUnpackRoundTripAndValidateHeader() {
    val iv = ByteArray(KeyBlob.IV_LEN) { (it + 1).toByte() }
    val ciphertext = ByteArray(KeyBlob.KEY_LEN + KeyBlob.TAG_LEN) { (it + 2).toByte() }

    val blob = KeyBlob.pack(iv, ciphertext)

    assertEquals(KeyBlob.BLOB_LEN, blob.size)
    assertEquals(KeyBlob.VERSION, blob[0])
    val unpacked = KeyBlob.unpack(blob)
    requireNotNull(unpacked)
    assertArrayEquals(iv, unpacked.first)
    assertArrayEquals(ciphertext, unpacked.second)
    assertNull(KeyBlob.unpack(blob.copyOf(KeyBlob.BLOB_LEN - 1)))
    assertNull(KeyBlob.unpack(blob.copyOf().also { it[0] = 2 }))
  }

  @Test
  fun wrapThenUnwrapReturnsOriginalKey() {
    val plain = ByteArray(KeyBlob.KEY_LEN) { (it * 3).toByte() }

    val wrapped = WrapCall.handle(1, plain, ::encrypt, ::decrypt)
    val blob = (wrapped as WrapCall.Outcome.Ok).bytes
    assertEquals(KeyBlob.BLOB_LEN, blob.size)

    val unwrapped = WrapCall.handle(2, blob, ::encrypt, ::decrypt)
    assertArrayEquals(plain, (unwrapped as WrapCall.Outcome.Ok).bytes)
  }

  @Test
  fun authenticationFailureAndWrongKeyAreRejected() {
    val plain = ByteArray(KeyBlob.KEY_LEN) { (it + 7).toByte() }
    val blob = (WrapCall.handle(1, plain, ::encrypt, ::decrypt) as WrapCall.Outcome.Ok).bytes

    val changed = blob.copyOf().also { it[20] = (it[20].toInt() xor 1).toByte() }
    assertEquals(WrapCall.Outcome.Rejected, WrapCall.handle(2, changed, ::encrypt, ::decrypt))

    val otherKey = SecretKeySpec(ByteArray(KeyBlob.KEY_LEN) { (it + 91).toByte() }, "AES")
    val wrongKeyResult = WrapCall.handle(
      2,
      blob,
      encrypt = { encryptWith(it, otherKey) },
      decrypt = { iv, ciphertext -> decryptWith(iv, ciphertext, otherKey) },
    )
    assertEquals(WrapCall.Outcome.Rejected, wrongKeyResult)
  }

  @Test
  fun invalidOperationInputAndUnavailableKeyAreMapped() {
    assertEquals(
      WrapCall.Outcome.Unavailable,
      WrapCall.handle(
        1,
        ByteArray(KeyBlob.KEY_LEN),
        encrypt = { throw WrapCall.KeyUnavailable() },
        decrypt = ::decrypt,
      ),
    )
    assertEquals(
      WrapCall.Outcome.Rejected,
      WrapCall.handle(9, ByteArray(KeyBlob.KEY_LEN), ::encrypt, ::decrypt),
    )
    assertEquals(
      WrapCall.Outcome.Rejected,
      WrapCall.handle(1, ByteArray(KeyBlob.KEY_LEN - 1), ::encrypt, ::decrypt),
    )
  }

  private fun encrypt(plain: ByteArray): Pair<ByteArray, ByteArray> = encryptWith(plain, key)

  private fun encryptWith(plain: ByteArray, secretKey: SecretKeySpec): Pair<ByteArray, ByteArray> {
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, secretKey)
    cipher.updateAAD(KeyBlob.AAD)
    return cipher.iv to cipher.doFinal(plain)
  }

  private fun decrypt(iv: ByteArray, ciphertext: ByteArray): ByteArray = decryptWith(iv, ciphertext, key)

  private fun decryptWith(
    iv: ByteArray,
    ciphertext: ByteArray,
    secretKey: SecretKeySpec,
  ): ByteArray {
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, secretKey, GCMParameterSpec(128, iv))
    cipher.updateAAD(KeyBlob.AAD)
    return cipher.doFinal(ciphertext)
  }
}
