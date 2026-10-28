package ai.gxlself.remotescreen.logic.gestures

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SpaceSwipeTest {
  private val width = 1000f

  @Test
  fun smallMovesStayATapAndVerticalOnesAreLeftAlone() {
    assertEquals(SpaceSwipe.Classification.Undecided, SpaceSwipe.classify(10f, 5f))
    assertEquals(SpaceSwipe.Classification.Swipe, SpaceSwipe.classify(-30f, 4f))
    assertEquals(SpaceSwipe.Classification.Other, SpaceSwipe.classify(10f, 40f))
    assertEquals(SpaceSwipe.Classification.Other, SpaceSwipe.classify(30f, 28f))
  }

  @Test
  fun pastAThirdOrAFlickSwitchesOtherwiseItStays() {
    assertEquals(SpaceSwipeDirection.Next, SpaceSwipe.decide(-340f, 0f, width, true, true))
    assertEquals(SpaceSwipeDirection.Previous, SpaceSwipe.decide(340f, 0f, width, true, true))
    assertNull(SpaceSwipe.decide(-320f, 0f, width, true, true))
    assertEquals(SpaceSwipeDirection.Next, SpaceSwipe.decide(-60f, -900f, width, false, true))
    assertNull(SpaceSwipe.decide(-10f, -900f, width, false, true))
    assertNull(SpaceSwipe.decide(-500f, 900f, width, true, true))
  }

  @Test
  fun theEndsNeverSwitchOrWrap() {
    assertNull(SpaceSwipe.decide(800f, 1200f, width, false, true))
    assertNull(SpaceSwipe.decide(-800f, -1200f, width, true, false))
    assertEquals(null to "b", SpaceSwipe.neighbours("a", listOf("a", "b", "c")))
    assertEquals("a" to "c", SpaceSwipe.neighbours("b", listOf("a", "b", "c")))
    assertEquals("b" to null, SpaceSwipe.neighbours("c", listOf("a", "b", "c")))
    assertEquals(null to null, SpaceSwipe.neighbours("x", listOf("a")))
  }

  @Test
  fun threeFingersMovingSidewaysBecomeASwipe() {
    val tracker = SpaceSwipeTracker()
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(1, 500f, 300f, 0))
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(3, 500f, 300f, 10))
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(3, 490f, 301f, 20))
    assertEquals(SpaceSwipeTracker.Step.Began, tracker.track(3, 460f, 302f, 30))
    assertTrue(tracker.swiping)
    assertEquals(SpaceSwipeTracker.Step.Consumed, tracker.track(3, 200f, 305f, 120))
    assertEquals(SpaceSwipeDirection.Next, tracker.release(width, hasPrevious = true, hasNext = true))
    // The rest of the gesture stays with the swipe.
    assertEquals(SpaceSwipeTracker.Step.Consumed, tracker.track(2, 190f, 305f, 130))
    tracker.reset()
    assertFalse(tracker.swiping)
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(1, 0f, 0f, 200))
  }

  @Test
  fun aThreeFingerTapIsNotASwipe() {
    val tracker = SpaceSwipeTracker()
    tracker.track(3, 500f, 300f, 0)
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(3, 505f, 303f, 50))
    assertEquals(SpaceSwipeTracker.Step.Pass, tracker.track(2, 505f, 303f, 60))
    assertNull(tracker.release(width, hasPrevious = true, hasNext = true))
  }

  @Test
  fun verticalAndFourFingerGesturesAreLeftAlone() {
    val vertical = SpaceSwipeTracker()
    vertical.track(3, 500f, 300f, 0)
    assertEquals(SpaceSwipeTracker.Step.Pass, vertical.track(3, 505f, 360f, 50))
    assertEquals(SpaceSwipeTracker.Step.Pass, vertical.track(3, 300f, 360f, 80))
    assertFalse(vertical.swiping)

    val four = SpaceSwipeTracker()
    four.track(3, 500f, 300f, 0)
    assertEquals(SpaceSwipeTracker.Step.Pass, four.track(4, 400f, 300f, 20))
    assertEquals(SpaceSwipeTracker.Step.Pass, four.track(4, 200f, 300f, 40))
    assertFalse(four.swiping)
  }

  @Test
  fun aShortFlickSwitchesAndASlowShortDragSpringsBack() {
    val flick = SpaceSwipeTracker()
    flick.track(3, 500f, 300f, 0)
    flick.track(3, 470f, 300f, 10)
    flick.track(3, 420f, 300f, 60)
    assertTrue(flick.velocity() < -600f)
    assertEquals(SpaceSwipeDirection.Next, flick.release(width, hasPrevious = false, hasNext = true))

    val slow = SpaceSwipeTracker()
    slow.track(3, 500f, 300f, 0)
    slow.track(3, 530f, 300f, 100)
    slow.track(3, 560f, 300f, 600)
    assertNull(slow.release(width, hasPrevious = true, hasNext = true))
  }
}
