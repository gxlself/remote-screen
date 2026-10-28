package ai.gxlself.remotescreen.logic.scanner

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScannerTests {
  @Test
  fun throttleSuppressesRepeatedTextForTwoSeconds() {
    val throttle = QrScanThrottle()
    assertTrue(throttle.accept("rsp://pair?a", 0))
    assertFalse(throttle.accept("rsp://pair?a", 1_999))
    assertTrue(throttle.accept("rsp://pair?a", 2_001))
    assertTrue(throttle.accept("rsp://pair?b", 2_002))
  }

  @Test
  fun throttleKeepsSeparateWindowsForDifferentCodes() {
    val throttle = QrScanThrottle()
    assertTrue(throttle.accept("rsp://pair?a", 0))
    assertTrue(throttle.accept("rsp://pair?b", 1))
    assertFalse(throttle.accept("rsp://pair?a", 2))
    assertTrue(throttle.accept("rsp://pair?a", 2_001))
  }

  @Test
  fun throttleRecoversAfterClockMovesBackwards() {
    val throttle = QrScanThrottle()
    assertTrue(throttle.accept("rsp://pair?a", 5_000))
    assertTrue(throttle.accept("rsp://pair?a", 1_000))
    assertFalse(throttle.accept("rsp://pair?a", 1_001))
  }

  @Test
  fun throttleUsesCustomWindow() {
    val throttle = QrScanThrottle(windowMs = 10)
    assertTrue(throttle.accept("a", 0))
    assertFalse(throttle.accept("a", 9))
    assertTrue(throttle.accept("a", 10))
  }

  @Test
  fun permissionDecisionPrioritizesMissingDevice() {
    assertEquals(ScannerPermissionDecision.START, ScannerPermission.decide(ScannerPermissionStatus.AUTHORIZED, true))
    assertEquals(ScannerPermissionDecision.REQUEST, ScannerPermission.decide(ScannerPermissionStatus.NOT_DETERMINED, true))
    assertEquals(ScannerPermissionDecision.DENIED, ScannerPermission.decide(ScannerPermissionStatus.DENIED, true))
    assertEquals(ScannerPermissionDecision.RESTRICTED, ScannerPermission.decide(ScannerPermissionStatus.RESTRICTED, true))
    ScannerPermissionStatus.entries.forEach { status ->
      assertEquals(ScannerPermissionDecision.UNAVAILABLE, ScannerPermission.decide(status, false))
    }
  }

  @Test
  fun androidPermissionMapsAllFourCombinations() {
    assertEquals(ScannerPermissionStatus.RESTRICTED, ScannerPermission.fromAndroid(false, true))
    assertEquals(ScannerPermissionStatus.RESTRICTED, ScannerPermission.fromAndroid(true, true))
    assertEquals(ScannerPermissionStatus.AUTHORIZED, ScannerPermission.fromAndroid(true, false))
    assertEquals(ScannerPermissionStatus.NOT_DETERMINED, ScannerPermission.fromAndroid(false, false))
  }

  @Test
  fun lumaPacksContiguousRows() {
    assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6), LumaPlane.pack(byteArrayOf(1, 2, 3, 4, 5, 6), 3, 1, 3, 2))
  }

  @Test
  fun lumaSkipsRowPadding() {
    assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6), LumaPlane.pack(byteArrayOf(1, 2, 3, 99, 4, 5, 6, 88), 4, 1, 3, 2))
  }

  @Test
  fun lumaSkipsInterleavedPixels() {
    assertArrayEquals(byteArrayOf(10, 12, 20, 22), LumaPlane.pack(byteArrayOf(10, 99, 12, 88, 20, 77, 22, 66), 4, 2, 2, 2))
  }

  @Test
  fun lumaHandlesEmptyDimensions() {
    assertArrayEquals(byteArrayOf(), LumaPlane.pack(byteArrayOf(), 0, 1, 0, 2))
  }

  @Test
  fun lumaPreservesSignedByteValues() {
    assertArrayEquals(byteArrayOf(-1, -128), LumaPlane.pack(byteArrayOf(-1, -128), 2, 1, 2, 1))
  }
}
