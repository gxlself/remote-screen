package ai.gxlself.remotescreen.logic.adapters

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AdapterIdsTest {
  @Test
  fun splitsAtTwoToTheSixtySecondUnsigned() {
    for (id in listOf(0L, 1L, (1L shl 62) - 1)) {
      assertFalse(id.toString(), AdapterIds.isAdapterId(id))
    }
    for (id in listOf(1L shl 62, (1L shl 62) + 7, Long.MIN_VALUE)) {
      assertTrue(id.toString(), AdapterIds.isAdapterId(id))
    }
  }
}
