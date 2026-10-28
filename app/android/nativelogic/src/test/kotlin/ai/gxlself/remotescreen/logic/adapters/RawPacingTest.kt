package ai.gxlself.remotescreen.logic.adapters

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RawPacingTest {
  @Test
  fun firstReadyImmediate() {
    val pacing = RawPacing()
    assertEquals(0L, pacing.ready(1, 1000))
    assertNull(pacing.ready(2, 1001))
  }

  @Test
  fun beginReturnsLatest() {
    val pacing = RawPacing()
    pacing.ready(1, 1000)
    pacing.ready(3, 1001)
    pacing.ready(2, 1002)
    assertEquals(3L, pacing.begin(1003))
  }

  @Test
  fun endIdleThenPaced() {
    val pacing = RawPacing()
    pacing.begin(1000)
    assertNull(pacing.end(3, 1004))
    assertEquals(11L, pacing.ready(4, 1005))
  }

  @Test
  fun endBehind() {
    val pacing = RawPacing()
    pacing.begin(1000)
    assertNull(pacing.ready(5, 1001))
    assertEquals(14L, pacing.end(4, 1002))

    val late = RawPacing()
    late.begin(1000)
    assertNull(late.ready(5, 1001))
    assertEquals(0L, late.end(4, 1030))
  }

  @Test
  fun endZero() {
    val pacing = RawPacing()
    pacing.ready(2, 1000)
    pacing.begin(1000)
    assertNull(pacing.ready(5, 1001))
    assertNull(pacing.end(0, 1002))
    assertEquals(14L, pacing.ready(6, 1002))
  }

  @Test
  fun unsignedSeq() {
    val pacing = RawPacing()
    pacing.ready(-1L, 1000)
    pacing.ready(5, 1001)
    assertEquals(-1L, pacing.begin(1002))
    assertEquals(16L, pacing.end(Long.MAX_VALUE, 1002))
  }

  @Test
  fun reset() {
    val used = RawPacing()
    used.ready(8, 1000)
    used.begin(1000)
    assertEquals(15L, used.end(7, 1001))
    assertNull(used.ready(9, 1002))
    used.reset()
    val fresh = script(RawPacing())
    assertEquals(listOf<Long?>(0, null, 2, 13, 2, null, 13), fresh)
    assertEquals(fresh, script(used))
  }

  private fun script(pacing: RawPacing): List<Long?> = listOf(
    pacing.ready(1, 1005),
    pacing.ready(2, 1006),
    pacing.begin(1007),
    pacing.end(1, 1010),
    pacing.begin(1023),
    pacing.end(2, 1025),
    pacing.ready(3, 1026),
  )
}
