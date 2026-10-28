package ai.gxlself.remotescreen.logic.common

import org.junit.Assert.assertEquals
import org.junit.Test

class MarkerLineTest {
  @Test
  fun sortsKeysAndEscapesStrings() {
    assertEquals(
      "RS-TEST {\"a\":\"line\\nbreak\",\"b\":\"x\\\"y\"}",
      MarkerLine.format("TEST", mapOf("b" to "x\"y", "a" to "line\nbreak")),
    )
  }

  @Test
  fun escapesBackslashAndControlCharacters() {
    assertEquals(
      "RS-T {\"value\":\"\\\\\\r\\t\\u0001\"}",
      MarkerLine.format("T", mapOf("value" to "\\\r\t\u0001")),
    )
  }

  @Test
  fun leavesSlashAndUnicodeUnescaped() {
    assertEquals(
      "RS-T {\"value\":\"/ café\"}",
      MarkerLine.format("T", mapOf("value" to "/ café")),
    )
  }

  @Test
  fun invalidTagsAreTruncated() {
    assertEquals("RS-INVALID {\"tag\":\"bad tag\"}", MarkerLine.format("bad tag", emptyMap()))
    assertEquals("RS-INVALID {\"tag\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\"}", MarkerLine.format("A".repeat(33), emptyMap()))
    assertEquals("RS-INVALID {\"tag\":\"\"}", MarkerLine.format("", emptyMap()))
  }

  @Test
  fun nonFiniteNumbersBecomeNull() {
    assertEquals(
      "RS-N {\"f\":null,\"i\":null}",
      MarkerLine.format("N", mapOf("f" to Double.POSITIVE_INFINITY, "i" to Float.NaN)),
    )
  }

  @Test
  fun nestedListsAreNormalizedRecursively() {
    assertEquals(
      "RS-ARRAY {\"value\":[1,[true,\"text\",null]]}",
      MarkerLine.format("ARRAY", mapOf("value" to listOf(1, listOf(true, "text", null)))),
    )
  }

  @Test
  fun supportsNullLongAndUnsupportedValues() {
    assertEquals(
      "RS-V {\"long\":9223372036854775807,\"null\":null,\"other\":\"<unsupported>\"}",
      MarkerLine.format("V", mapOf("null" to null, "long" to Long.MAX_VALUE, "other" to Any())),
    )
  }
}
