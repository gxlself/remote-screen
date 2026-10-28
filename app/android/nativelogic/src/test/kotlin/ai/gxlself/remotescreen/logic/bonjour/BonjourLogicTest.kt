package ai.gxlself.remotescreen.logic.bonjour

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BonjourLogicTest {
  @Test
  fun addressesAreUniqueIpv4FirstAndLimitedToFour() {
    assertEquals(
      listOf("10.0.0.2", "172.16.0.1", "192.168.1.2", "2001:db8::1"),
      BonjourLogic.sortAndLimitAddresses(
        listOf(
          "fe80::1",
          "192.168.1.2",
          "2001:db8::1",
          "10.0.0.2",
          "192.168.1.2",
          "",
          "172.16.0.1",
        ),
      ),
    )
  }

  @Test
  fun hostUsesTheBonjourServiceName() {
    assertEquals("Mac-mini.local", BonjourLogic.hostForServiceName("Mac-mini"))
  }

  @Test
  fun resolveQueueIsFifoAndDeduplicatesPendingKeys() {
    val queue = BonjourResolveQueue<String>()
    assertTrue(queue.enqueue("one"))
    assertFalse(queue.enqueue("one"))
    assertTrue(queue.enqueue("two"))
    assertEquals("one", queue.startNext())
    assertEquals(1, queue.pendingCount)
    assertEquals(null, queue.startNext())
    assertTrue(queue.complete("one"))
    assertEquals("two", queue.startNext())
    assertFalse(queue.complete("one"))
  }

  @Test
  fun serviceDeduplicatorAllowsChangedResolutionAndRemovesOnLost() {
    val deduplicator = BonjourServiceDeduplicator<String>()
    val first = BonjourService("Mac", "Mac.local", 5900, listOf("192.168.1.2"))
    val changed = first.copy(addrs = listOf("192.168.1.2", "192.168.1.3"))
    assertTrue(deduplicator.shouldEmitFound("Mac", first))
    assertFalse(deduplicator.shouldEmitFound("Mac", first))
    assertTrue(deduplicator.shouldEmitFound("Mac", changed))
    assertEquals(changed, deduplicator.remove("Mac"))
    assertEquals(null, deduplicator.remove("Mac"))
  }
}
