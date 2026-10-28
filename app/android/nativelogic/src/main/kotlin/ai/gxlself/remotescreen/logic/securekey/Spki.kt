package ai.gxlself.remotescreen.logic.securekey

import java.math.BigInteger
import java.security.MessageDigest

/** Helpers for the DER SubjectPublicKeyInfo form of a P-256 public key. */
object Spki {
  /** DER AlgorithmIdentifier and uncompressed EC point header for secp256r1. */
  val P256_PREFIX: ByteArray =
    byteArrayOf(
      0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86.toByte(), 0x48,
      0xce.toByte(), 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86.toByte(), 0x48,
      0xce.toByte(), 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    )

  /** Wrap an X9.63 uncompressed point in a P-256 SPKI. */
  fun fromX963(x963: ByteArray): ByteArray {
    require(x963.size == 65 && x963[0] == 0x04.toByte()) {
      "P-256 X9.63 public key must be 65 bytes and start with 0x04"
    }
    return P256_PREFIX + x963
  }

  /** Encode affine coordinates as an uncompressed X9.63 point. */
  fun x963FromAffine(x: BigInteger, y: BigInteger): ByteArray {
    return byteArrayOf(0x04) + coordinate(x) + coordinate(y)
  }

  fun isP256Spki(der: ByteArray): Boolean {
    return der.size == 91 &&
      der.copyOfRange(0, P256_PREFIX.size).contentEquals(P256_PREFIX) &&
      der[P256_PREFIX.size] == 0x04.toByte()
  }

  fun fingerprint(spki: ByteArray): ByteArray =
    MessageDigest.getInstance("SHA-256").digest(spki)

  fun formatFingerprint(fp: ByteArray): String = buildString {
    append("SHA256 ")
    fp.forEachIndexed { index, byte ->
      if (index > 0) append(':')
      append("%02X".format(byte.toInt() and 0xff))
    }
  }

  /** The first ten fingerprint bytes encoded as unpadded RFC 4648 base32. */
  fun deviceId(fp: ByteArray): String {
    if (fp.isEmpty()) return ""
    val alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
    val bytes = fp.copyOf(minOf(10, fp.size))
    val result = StringBuilder((bytes.size * 8 + 4) / 5)
    var buffer = 0
    var bits = 0
    for (byte in bytes) {
      buffer = (buffer shl 8) or (byte.toInt() and 0xff)
      bits += 8
      while (bits >= 5) {
        bits -= 5
        result.append(alphabet[(buffer ushr bits) and 0x1f])
      }
    }
    if (bits > 0) result.append(alphabet[(buffer shl (5 - bits)) and 0x1f])
    return result.toString().lowercase()
  }

  private fun coordinate(value: BigInteger): ByteArray {
    require(value.signum() >= 0) { "P-256 affine coordinates cannot be negative" }
    val raw = value.toByteArray()
    val withoutSign = if (raw.size > 1 && raw[0] == 0.toByte()) {
      raw.copyOfRange(1, raw.size)
    } else {
      raw
    }
    require(withoutSign.size <= 32) { "P-256 affine coordinate exceeds 32 bytes" }
    return ByteArray(32 - withoutSign.size) + withoutSign
  }
}
