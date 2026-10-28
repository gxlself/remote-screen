package ai.gxlself.remotescreen.logic.recording

import java.security.GeneralSecurityException

/** The opaque AES-GCM envelope used for recording file keys. */
object KeyBlob {
  const val VERSION: Byte = 1
  const val KEY_LEN = 32
  const val IV_LEN = 12
  const val TAG_LEN = 16
  const val BLOB_LEN = 1 + IV_LEN + KEY_LEN + TAG_LEN
  val AAD: ByteArray = "rs.recording.v1".toByteArray(Charsets.US_ASCII)

  /** Packs version || iv || (ciphertext || tag). */
  fun pack(iv: ByteArray, ctWithTag: ByteArray): ByteArray {
    require(iv.size == IV_LEN) { "recording key IV must be $IV_LEN bytes" }
    require(ctWithTag.size == KEY_LEN + TAG_LEN) {
      "recording key ciphertext must be ${KEY_LEN + TAG_LEN} bytes"
    }
    return ByteArray(BLOB_LEN).also { blob ->
      blob[0] = VERSION
      iv.copyInto(blob, destinationOffset = 1)
      ctWithTag.copyInto(blob, destinationOffset = 1 + IV_LEN)
    }
  }

  /** Returns the IV and ciphertext-with-tag, or null for an invalid envelope. */
  fun unpack(blob: ByteArray): Pair<ByteArray, ByteArray>? {
    if (blob.size != BLOB_LEN || blob[0] != VERSION) return null
    return blob.copyOfRange(1, 1 + IV_LEN) to
      blob.copyOfRange(1 + IV_LEN, BLOB_LEN)
  }
}

/** Pure callback protocol logic shared by Android production code and JVM tests. */
object WrapCall {
  const val OK = 0
  const val UNAVAILABLE = -1
  const val REJECTED = -2

  sealed interface Outcome {
    data class Ok(val bytes: ByteArray) : Outcome
    data object Unavailable : Outcome
    data object Rejected : Outcome
  }

  /**
   * Applies the wrap/unwrap protocol using injected cryptographic operations.
   * A key lookup failure is unavailable; authentication and other crypto failures
   * reject the request.
   */
  fun handle(
    op: Int,
    input: ByteArray,
    encrypt: (ByteArray) -> Pair<ByteArray, ByteArray>,
    decrypt: (ByteArray, ByteArray) -> ByteArray,
  ): Outcome {
    return when (op) {
      1 -> {
        if (input.size != KeyBlob.KEY_LEN) return Outcome.Rejected
        try {
          val (iv, ctWithTag) = encrypt(input)
          Outcome.Ok(KeyBlob.pack(iv, ctWithTag))
        } catch (_: KeyUnavailable) {
          Outcome.Unavailable
        } catch (_: GeneralSecurityException) {
          Outcome.Rejected
        } catch (_: IllegalArgumentException) {
          Outcome.Rejected
        }
      }

      2 -> {
        if (input.size != KeyBlob.BLOB_LEN) return Outcome.Rejected
        val (iv, ctWithTag) = KeyBlob.unpack(input) ?: return Outcome.Rejected
        try {
          val plain = decrypt(iv, ctWithTag)
          if (plain.size != KeyBlob.KEY_LEN) Outcome.Rejected else Outcome.Ok(plain)
        } catch (_: KeyUnavailable) {
          Outcome.Unavailable
        } catch (_: GeneralSecurityException) {
          Outcome.Rejected
        } catch (_: IllegalArgumentException) {
          Outcome.Rejected
        }
      }

      else -> Outcome.Rejected
    }
  }

  class KeyUnavailable(cause: Throwable? = null) : Exception(cause)
}
