package ai.gxlself.remotescreen.logic.pip

import org.junit.Assert.assertEquals
import org.junit.Test

class PipLogicTest {
  @Test
  fun keepsLandscapeSixteenByNine() {
    assertEquals(16 to 9, PipAspect.clamp(16, 9))
  }

  @Test
  fun keepsFourByThree() {
    assertEquals(4 to 3, PipAspect.clamp(4, 3))
  }

  @Test
  fun clampsTooWideRatio() {
    assertEquals(239 to 100, PipAspect.clamp(1000, 100))
  }

  @Test
  fun clampsTooTallRatio() {
    assertEquals(100 to 239, PipAspect.clamp(100, 1000))
  }

  @Test
  fun usesSixteenByNineForZeroDimensions() {
    assertEquals(16 to 9, PipAspect.clamp(0, 0))
  }

  @Test
  fun usesSixteenByNineForNegativeDimensions() {
    assertEquals(16 to 9, PipAspect.clamp(-100, 200))
  }

  @Test
  fun entersOnLeaveHintOnlyBeforeAndroidT() {
    assertEquals(true, PipPolicy.enterOnLeaveHint(30, eligible = true, supported = true))
    assertEquals(false, PipPolicy.enterOnLeaveHint(31, eligible = true, supported = true))
  }

  @Test
  fun requiresEligibilityAndSupport() {
    assertEquals(false, PipPolicy.enterOnLeaveHint(30, eligible = false, supported = true))
    assertEquals(false, PipPolicy.enterOnLeaveHint(30, eligible = true, supported = false))
    assertEquals(false, PipPolicy.enterOnLeaveHint(30, eligible = false, supported = false))
  }
}
