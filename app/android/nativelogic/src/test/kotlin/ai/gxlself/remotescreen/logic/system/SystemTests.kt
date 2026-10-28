package ai.gxlself.remotescreen.logic.system

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SystemTests {
  @Test
  fun parsesAllHapticKinds() {
    assertEquals(HapticKind.LIGHT, HapticKind.parse("light"))
    assertEquals(HapticKind.SUCCESS, HapticKind.parse("success"))
    assertEquals(HapticKind.WARNING, HapticKind.parse("warning"))
    assertEquals(HapticKind.ERROR, HapticKind.parse("error"))
  }

  @Test
  fun rejectsUnknownHapticSpellings() {
    assertNull(HapticKind.parse("Light"))
    assertNull(HapticKind.parse(""))
    assertNull(HapticKind.parse("heavy"))
  }

  @Test
  fun mapsHapticsForAndroid29() {
    assertEquals(3, HapticMap.constantFor(HapticKind.LIGHT, 29))
    assertEquals(1, HapticMap.constantFor(HapticKind.SUCCESS, 29))
    assertEquals(0, HapticMap.constantFor(HapticKind.WARNING, 29))
    assertEquals(0, HapticMap.constantFor(HapticKind.ERROR, 29))
  }

  @Test
  fun mapsHapticsForAndroid30() {
    assertEquals(3, HapticMap.constantFor(HapticKind.LIGHT, 30))
    assertEquals(16, HapticMap.constantFor(HapticKind.SUCCESS, 30))
    assertEquals(0, HapticMap.constantFor(HapticKind.WARNING, 30))
    assertEquals(17, HapticMap.constantFor(HapticKind.ERROR, 30))
  }

  @Test
  fun notificationSettingsDisabledAlwaysDeny() {
    assertEquals("denied", NotificationStatus.status(32, true, false, false))
    assertEquals("denied", NotificationStatus.status(34, true, false, false))
  }

  @Test
  fun preAndroid13DoesNotNeedRuntimePermission() {
    assertEquals("granted", NotificationStatus.status(32, false, true, false))
    assertEquals("granted", NotificationStatus.status(32, false, true, true))
  }

  @Test
  fun Android13PermissionStateIsMapped() {
    assertEquals("granted", NotificationStatus.status(33, true, true, true))
    assertEquals("notDetermined", NotificationStatus.status(33, false, true, false))
    assertEquals("denied", NotificationStatus.status(33, false, true, true))
  }

  @Test
  fun maxFpsRoundsTheHighestFiniteRate() {
    assertEquals(120, MaxFps.maxFps(listOf(60f, 120f, 90f)))
    assertEquals(60, MaxFps.maxFps(listOf(59.94f)))
  }

  @Test
  fun maxFpsIgnoresNonFiniteRatesWhenFiniteRatesExist() {
    assertEquals(90, MaxFps.maxFps(listOf(Float.NaN, 89.5f, 90f, Float.POSITIVE_INFINITY)))
  }

  @Test
  fun maxFpsUsesDefaultForEmptyOrNonFiniteRates() {
    assertEquals(60, MaxFps.maxFps(emptyList()))
    assertEquals(
      60,
      MaxFps.maxFps(listOf(Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY)),
    )
  }
}
