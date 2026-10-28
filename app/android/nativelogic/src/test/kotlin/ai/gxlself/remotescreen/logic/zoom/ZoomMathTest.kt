package ai.gxlself.remotescreen.logic.zoom

import ai.gxlself.remotescreen.logic.gestures.GPoint
import ai.gxlself.remotescreen.logic.gestures.GRect
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ZoomMathTest {
  @Test
  fun clampOffsetAtOneZoom() {
    assertPoint(GPoint.ZERO, ZoomMath.clampOffset(1f, GPoint(-1f, 2f)))
  }

  @Test
  fun clampOffsetAtFourZoom() {
    assertPoint(GPoint(0.75f, 0.75f), ZoomMath.clampOffset(4f, GPoint(1f, 1f)))
  }

  @Test
  fun zoomCenterKeepsCenterFixed() {
    val result = ZoomMath.zoomed(Viewport(), 2f, GPoint(0.5f, 0.5f))
    assertEquals(2f, result.zoom, EPSILON)
    assertPoint(GPoint(0.25f, 0.25f), result.offset)
  }

  @Test
  fun zoomCornerKeepsCornerAtCorner() {
    val result = ZoomMath.zoomed(Viewport(), 4f, GPoint.ZERO)
    assertPoint(GPoint.ZERO, result.offset)
  }

  @Test
  fun zoomOverMaxIsClamped() {
    val result = ZoomMath.zoomed(Viewport(3f, GPoint(0.1f, 0.1f)), 4f, GPoint(0.5f, 0.5f))
    assertEquals(4f, result.zoom, EPSILON)
    assertTrue(result.offset.x in 0f..0.75f)
  }

  @Test
  fun zoomBelowMinimumIsClamped() {
    val result = ZoomMath.zoomed(Viewport(2f, GPoint(0.2f, 0.2f)), 0.01f, GPoint(0.5f, 0.5f))
    assertEquals(1f, result.zoom, EPSILON)
    assertPoint(GPoint.ZERO, result.offset)
  }

  @Test
  fun pannedClampsBothDirections() {
    val result = ZoomMath.panned(Viewport(2f, GPoint(0.25f, 0.25f)), GPoint(1f, -1f))
    assertPoint(GPoint(0f, 0.5f), result.offset)
  }

  @Test
  fun pannedNegativeDeltaMovesTowardEnd() {
    val result = ZoomMath.panned(Viewport(2f, GPoint.ZERO), GPoint(-1f, -1f))
    assertPoint(GPoint(0.5f, 0.5f), result.offset)
  }

  @Test
  fun followCursorDoesNotMoveInMiddle() {
    val original = Viewport(2f, GPoint(0.25f, 0.25f))
    assertEquals(original, ZoomMath.followCursor(original, GPoint(0.5f, 0.5f)))
  }

  @Test
  fun followCursorMovesLeftAndUpWhenNearTopLeft() {
    val result = ZoomMath.followCursor(Viewport(2f, GPoint(0.25f, 0.25f)), GPoint(0.25f, 0.25f))
    assertPoint(GPoint(0.2f, 0.2f), result.offset)
  }

  @Test
  fun followCursorMovesRightAndDownWhenNearBottomRight() {
    val result = ZoomMath.followCursor(Viewport(2f, GPoint.ZERO), GPoint(0.75f, 0.75f))
    assertPoint(GPoint(0.3f, 0.3f), result.offset)
  }

  @Test
  fun followCursorClampsAtRemoteCorners() {
    val result = ZoomMath.followCursor(Viewport(4f, GPoint.ZERO), GPoint(1f, 1f))
    assertPoint(GPoint(0.75f, 0.75f), result.offset)
  }

  @Test
  fun keyboardTargetZoomsAndCentersAboveInset() {
    val result = ZoomMath.keyboardTarget(Viewport(), GPoint(0.5f, 0.5f), 0.4f)
    assertEquals(1.7f, result.zoom, EPSILON)
    assertEquals((0.5f - (1f / 1.7f) / 2f), result.offset.x, EPSILON)
    assertEquals((0.5f - (1f / 1.7f) * 0.6f / 2f), result.offset.y, EPSILON)
  }

  @Test
  fun keyboardTargetDoesNotZoomExistingLargerViewport() {
    val result = ZoomMath.keyboardTarget(Viewport(2f, GPoint.ZERO), GPoint(0.5f, 0.5f), 0.25f)
    assertEquals(2f, result.zoom, EPSILON)
  }

  @Test
  fun keyboardTargetCornerClamps() {
    val result = ZoomMath.keyboardTarget(Viewport(), GPoint(1f, 1f), 0.8f)
    assertTrue(result.offset.x in 0f..(1f - 1f / result.zoom))
    assertTrue(result.offset.y in 0f..(1f - 1f / result.zoom))
  }

  @Test
  fun touchTapMapperClampsOutsideContent() {
    val result = TouchTapMapper.normalized(
      GPoint(-10f, 500f),
      GRect(10f, 20f, 100f, 200f),
      Viewport(),
    )
    assertPoint(GPoint.ZERO.copy(y = 1f), result)
  }

  @Test
  fun touchTapMapperAccountsForZoomOffset() {
    val result = TouchTapMapper.normalized(
      GPoint(50f, 50f),
      GRect(0f, 0f, 100f, 100f),
      Viewport(2f, GPoint(0.25f, 0.1f)),
    )
    assertPoint(GPoint(0.5f, 0.35f), result)
  }

  @Test
  fun magnifierUsesAbovePositionWhenAvailable() {
    assertPoint(GPoint(100f, 120f), MagnifierPlacement.center(GPoint(100f, 200f), 800f))
  }

  @Test
  fun magnifierMovesBelowNearTop() {
    assertPoint(GPoint(100f, 100f), MagnifierPlacement.center(GPoint(100f, 20f), 800f))
  }

  @Test
  fun emitterThrottlesAndFlushesLatest() {
    val emitter = ViewportEmitter()
    val first = Viewport(1f, GPoint.ZERO)
    val second = Viewport(2f, GPoint(0.25f, 0.25f))
    assertEquals(first, emitter.offer(first, 0))
    assertNull(emitter.offer(second, 50))
    assertEquals(second, emitter.flush())
  }

  @Test
  fun emitterSkipsEqualValues() {
    val emitter = ViewportEmitter()
    val value = Viewport(2f, GPoint(0.25f, 0.25f))
    assertEquals(value, emitter.offer(value, 0))
    assertNull(emitter.offer(value, 100))
    assertNull(emitter.flush())
  }

  @Test
  fun emitterAllowsNewValueAfterInterval() {
    val emitter = ViewportEmitter()
    val first = Viewport(1f, GPoint.ZERO)
    val second = Viewport(2f, GPoint.ZERO)
    assertEquals(first, emitter.offer(first, 0))
    assertEquals(second, emitter.offer(second, 100))
  }

  private fun assertPoint(expected: GPoint, actual: GPoint) {
    assertEquals(expected.x, actual.x, EPSILON)
    assertEquals(expected.y, actual.y, EPSILON)
  }

  companion object {
    private const val EPSILON = 0.00001f
  }
}
