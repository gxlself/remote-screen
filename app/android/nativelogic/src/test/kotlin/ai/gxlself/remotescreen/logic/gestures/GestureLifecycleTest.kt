package ai.gxlself.remotescreen.logic.gestures

import java.util.Random
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GestureLifecycleTest {
  @Test
  fun attachDetachAttachKeepsReusableResourcesOpen() {
    val lifecycle = GestureLifecycle()

    val firstAttach = lifecycle.windowAttached()
    assertEquals(
      setOf(
        GestureLifecycle.Action.RegisterHooks,
        GestureLifecycle.Action.BindSession,
        GestureLifecycle.Action.RestoreEdgeExclusion,
      ),
      firstAttach.asSet(),
    )
    assertEquals(GestureLifecycle.State.Active, lifecycle.state)

    val detach = lifecycle.windowDetached()
    assertEquals(
      setOf(
        GestureLifecycle.Action.ReleaseInputs,
        GestureLifecycle.Action.CancelGestures,
        GestureLifecycle.Action.RecycleVelocityTracker,
        GestureLifecycle.Action.ClearEdgeExclusion,
        GestureLifecycle.Action.UnregisterHooks,
      ),
      detach.asSet(),
    )
    assertEquals(GestureLifecycle.State.Paused, lifecycle.state)

    val secondAttach = lifecycle.windowAttached()
    assertEquals(firstAttach.asSet(), secondAttach.asSet())
    assertEquals(GestureLifecycle.State.Active, lifecycle.state)
    assertTrue(lifecycle.hooksRegistered)
  }

  @Test
  fun detachDropClosesOnceAfterHooksAreRemoved() {
    val lifecycle = GestureLifecycle()
    lifecycle.windowAttached()
    lifecycle.windowDetached()

    val drop = lifecycle.drop()
    assertEquals(setOf(GestureLifecycle.Action.CloseResources), drop.asSet())
    assertEquals(GestureLifecycle.State.Closed, lifecycle.state)
    assertFalse(lifecycle.hooksRegistered)

    val secondDrop = lifecycle.drop()
    assertTrue(secondDrop.isEmpty)
  }

  @Test
  fun dropFromActiveIncludesPauseActions() {
    val lifecycle = GestureLifecycle()
    lifecycle.windowAttached()

    val drop = lifecycle.drop()
    assertEquals(
      setOf(
        GestureLifecycle.Action.ReleaseInputs,
        GestureLifecycle.Action.CancelGestures,
        GestureLifecycle.Action.RecycleVelocityTracker,
        GestureLifecycle.Action.ClearEdgeExclusion,
        GestureLifecycle.Action.UnregisterHooks,
        GestureLifecycle.Action.CloseResources,
      ),
      drop.asSet(),
    )
    assertEquals(GestureLifecycle.State.Closed, lifecycle.state)
  }

  @Test
  fun dropThenAttachDoesNothing() {
    val lifecycle = GestureLifecycle()
    lifecycle.drop()

    val attach = lifecycle.windowAttached()
    assertTrue(attach.isEmpty)
    assertEquals(GestureLifecycle.State.Closed, lifecycle.state)
  }

  @Test
  fun repeatedDetachDoesNothingAfterFirstPause() {
    val lifecycle = GestureLifecycle()
    lifecycle.windowAttached()

    val first = lifecycle.windowDetached()
    val second = lifecycle.windowDetached()
    assertEquals(
      setOf(
        GestureLifecycle.Action.ReleaseInputs,
        GestureLifecycle.Action.CancelGestures,
        GestureLifecycle.Action.RecycleVelocityTracker,
        GestureLifecycle.Action.ClearEdgeExclusion,
        GestureLifecycle.Action.UnregisterHooks,
      ),
      first.asSet(),
    )
    assertTrue(second.isEmpty)
    assertEquals(GestureLifecycle.State.Paused, lifecycle.state)
  }

  @Test
  fun randomEventSequencesNeverDuplicateHooksOrClose() {
    val random = Random(0x4753544c)
    repeat(500) {
      val lifecycle = GestureLifecycle()
      var hooksRegistered = false
      var closeCount = 0
      repeat(40) {
        val actions = when (random.nextInt(3)) {
          0 -> lifecycle.windowAttached()
          1 -> lifecycle.windowDetached()
          else -> lifecycle.drop()
        }

        if (actions.registerHooks) {
          assertFalse(hooksRegistered)
          hooksRegistered = true
        }
        if (actions.unregisterHooks) {
          assertTrue(hooksRegistered)
          hooksRegistered = false
        }
        if (actions.closeResources) closeCount += 1
        assertTrue("at most one hook registration can be active", !hooksRegistered || lifecycle.hooksRegistered)
        assertTrue("resources close at most once", closeCount <= 1)
        if (lifecycle.state == GestureLifecycle.State.Closed) {
          assertTrue("closed events return no actions", actions.isEmpty || actions.closeResources)
          assertFalse(lifecycle.hooksRegistered)
          assertTrue(lifecycle.windowAttached().isEmpty)
          assertTrue(lifecycle.windowDetached().isEmpty)
          assertTrue(lifecycle.drop().isEmpty)
        }
      }
    }
  }
}
