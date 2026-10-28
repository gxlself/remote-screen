package ai.gxlself.remotescreen.logic.launch

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LaunchArgsTest {
  @Test
  fun recognizesAllValueKeysInDeclaredOrder() {
    val values = LaunchArgs.VALUE_KEYS.associateWith { "value-$it" }
    assertEquals(values, LaunchArgs.properties(values))
  }

  @Test
  fun skipsNullEmptyAndUnknownExtras() {
    assertEquals(
      mapOf("devHarness" to "ping"),
      LaunchArgs.properties(mapOf("devHarness" to "ping", "harnessArg" to "", "autoConnect" to null, "unknown" to "x")),
    )
  }

  @Test
  fun ignoresRemovedHubAndFakeDirectoryExtras() {
    assertEquals(
      emptyMap<String, String>(),
      LaunchArgs.properties(mapOf("hubUrl" to "http://100.64.0.10:7300", "fakeDirectory" to "fixtures.json")),
    )
  }

  @Test
  fun preservesWhitespaceValues() {
    assertEquals(mapOf("harnessArg" to " "), LaunchArgs.properties(mapOf("harnessArg" to " ")))
  }

  @Test
  fun dataDirEndsInRemoteScreenCore() {
    assertTrue(LaunchDataDir.dir(File("/data/no_backup")).path.endsWith("RemoteScreen${File.separator}core"))
  }
}
