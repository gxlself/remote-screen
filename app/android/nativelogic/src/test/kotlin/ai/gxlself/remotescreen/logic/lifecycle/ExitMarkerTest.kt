package ai.gxlself.remotescreen.logic.lifecycle

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ExitMarkerTest {
  @Test
  fun startReadsAndClearsBackgroundMarker() {
    val storage = InMemoryKeyValueBool()
    val marker = ExitMarker(storage)
    marker.markBackground()
    assertTrue(marker.start())
    assertFalse(marker.start())
  }

  @Test
  fun foregroundClearsMarker() {
    val storage = InMemoryKeyValueBool()
    val marker = ExitMarker(storage)
    marker.markBackground()
    marker.markForeground()
    assertFalse(marker.start())
  }
}
