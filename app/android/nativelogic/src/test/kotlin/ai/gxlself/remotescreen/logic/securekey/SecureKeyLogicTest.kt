package ai.gxlself.remotescreen.logic.securekey

import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.spec.ECGenParameterSpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SecureKeyLogicTest {
  @Test
  fun deviceIdUsesUnpaddedLowercaseBase32() {
    assertEquals("aaaqeayeaudaocaj", Spki.deviceId((0 until 32).map(Int::toByte).toByteArray()))
    assertEquals("", Spki.deviceId(byteArrayOf()))
  }

  @Test
  fun fingerprintFormattingMatchesIos() {
    val fp = (0 until 32).map(Int::toByte).toByteArray()
    assertEquals(
      "SHA256 " + (0 until 32).joinToString(":") { "%02X".format(it) },
      Spki.formatFingerprint(fp),
    )
  }

  @Test
  fun x963VectorHasExpectedSpkiAndFingerprint() {
    val x963 = byteArrayOf(0x04) + (1..64).map(Int::toByte).toByteArray()
    val spki = Spki.fromX963(x963)
    assertEquals(91, spki.size)
    assertEquals(
      "1558b216d7bfd33d7775c455d60380d3a2cf61f1fd980e9a34838559062bd5de",
      Spki.fingerprint(spki).joinToString("") { "%02x".format(it) },
    )
    assertEquals("cvmlefwxx7jt253v", Spki.deviceId(Spki.fingerprint(spki)))
    assertTrue(Spki.isP256Spki(spki))
  }

  @Test
  fun invalidX963IsRejected() {
    val wrongLength = ByteArray(64) { 0x04 }
    val wrongPrefix = byteArrayOf(0x02) + ByteArray(64)
    expectIllegalArgument { Spki.fromX963(wrongLength) }
    expectIllegalArgument { Spki.fromX963(wrongPrefix) }
  }

  @Test
  fun jdkP256PublicKeyRoundTripsThroughAffineCoordinates() {
    val generator = KeyPairGenerator.getInstance("EC")
    generator.initialize(ECGenParameterSpec("secp256r1"))
    val publicKey = generator.generateKeyPair().public
    val ec = publicKey as java.security.interfaces.ECPublicKey
    val spki = Spki.fromX963(Spki.x963FromAffine(ec.w.affineX, ec.w.affineY))
    assertEquals(91, publicKey.encoded.size)
    assertTrue(Spki.isP256Spki(publicKey.encoded))
    assertArrayEquals(publicKey.encoded, spki)
  }

  @Test
  fun affineCoordinatesAreLeftPadded() {
    val x = BigInteger("0102", 16)
    val y = BigInteger("a0", 16)
    val encoded = Spki.x963FromAffine(x, y)
    assertEquals(65, encoded.size)
    assertEquals(0x04.toByte(), encoded[0])
    assertEquals(0x01.toByte(), encoded[31])
    assertEquals(0x02.toByte(), encoded[32])
    assertEquals(0xa0.toByte(), encoded[64])
  }

  @Test
  fun oversizedOrNegativeAffineCoordinateIsRejected() {
    expectIllegalArgument { Spki.x963FromAffine(BigInteger.ONE.shiftLeft(256), BigInteger.ZERO) }
    expectIllegalArgument { Spki.x963FromAffine(BigInteger.valueOf(-1), BigInteger.ZERO) }
  }

  @Test
  fun spkiValidationChecksLengthAndPointMarker() {
    val valid = Spki.fromX963(byteArrayOf(0x04) + ByteArray(64))
    assertTrue(Spki.isP256Spki(valid))
    assertFalse(Spki.isP256Spki(valid.copyOf(90)))
    valid[26] = 0x03
    assertFalse(Spki.isP256Spki(valid))
  }

  @Test
  fun rotationTruthTableMatchesIos() {
    assertEquals(RotationAction.USE_EXISTING, Rotation.decide(true, true))
    assertEquals(RotationAction.CREATE, Rotation.decide(true, false))
    assertEquals(RotationAction.DELETE_AND_CREATE, Rotation.decide(false, true))
    assertEquals(RotationAction.CREATE, Rotation.decide(false, false))
  }

  @Test
  fun signStatsUsesNearestRankAndRounds() {
    val summary = SignStats.summary((1..100).map(Int::toDouble))
    assertEquals(50.0, summary.p50, 0.0)
    assertEquals(95.0, summary.p95, 0.0)
    assertEquals(100.0, summary.max, 0.0)
    assertEquals(SignStats.Summary(0.0, 0.0, 0.0), SignStats.summary(emptyList()))
    assertEquals(1.23, SignStats.round2(1.234), 0.0)
  }

  private fun expectIllegalArgument(block: () -> Unit) {
    try {
      block()
      throw AssertionError("expected IllegalArgumentException")
    } catch (_: IllegalArgumentException) {
      // expected
    }
  }
}
