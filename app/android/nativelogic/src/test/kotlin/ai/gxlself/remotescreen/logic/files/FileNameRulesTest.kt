package ai.gxlself.remotescreen.logic.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FileNameRulesTest {
  @Test
  fun removesPathSeparatorsAndControlCharacters() {
    assertEquals("foldernamepart.txt", FileNameRules.sanitize("folder/name\\part\u0000.txt"))
    assertEquals("abc", FileNameRules.sanitize("a\nb\rc\u007f\u0085"))
  }

  @Test
  fun addsIncreasingSuffixBeforeTheLastExtension() {
    assertEquals("a (2).txt", FileNameRules.unique("a.txt", setOf("a.txt")))
    assertEquals("a (3).txt", FileNameRules.unique("a.txt", setOf("a.txt", "a (2).txt")))
    assertEquals("archive.tar (2).gz", FileNameRules.unique("archive.tar.gz", setOf("archive.tar.gz")))
    assertEquals(".hidden (2)", FileNameRules.unique(".hidden", setOf(".hidden")))
    assertEquals("name (2)", FileNameRules.unique("na/me", setOf("name")))
    assertEquals("a.txt", FileNameRules.unique("a.txt", emptySet()))
  }

  @Test
  fun truncatesLongNamesAndKeepsExtension() {
    val result = FileNameRules.sanitize("x".repeat(400) + ".rspair")

    assertEquals("x".repeat(248) + ".rspair", result)
  }

  @Test(timeout = 1000)
  fun longNameCollisionsPreserveTheCounterWithinTheByteLimit() {
    val name = "x".repeat(400) + ".rspair"
    val taken = mutableSetOf(FileNameRules.sanitize(name))
    for (index in 2..12) {
      val result = FileNameRules.unique(name, taken)
      assertTrue(result.endsWith(" ($index).rspair"))
      assertTrue(result.toByteArray(Charsets.UTF_8).size <= FileNameRules.MAX_NAME_BYTES)
      taken.add(result)
    }
  }

  @Test
  fun truncatesUnicodeOnCodePointBoundaries() {
    val point = String(Character.toChars(0x1f4c4))
    val result = FileNameRules.sanitize(point.repeat(100) + ".txt")
    assertEquals(point.repeat(62) + ".txt", result)
    assertTrue(result.toByteArray(Charsets.UTF_8).size <= FileNameRules.MAX_NAME_BYTES)
  }

  @Test(timeout = 1000)
  fun veryLongExtensionsStillLeaveRoomForTheCounter() {
    val name = "x." + "y".repeat(400)
    val base = FileNameRules.sanitize(name)
    val collision = FileNameRules.unique(name, setOf(base))
    assertTrue(collision.startsWith("x (2)."))
    assertTrue(collision.toByteArray(Charsets.UTF_8).size <= FileNameRules.MAX_NAME_BYTES)
  }

  @Test
  fun emptyAndDirectoryNamesUseFallback() {
    for (name in listOf(null, "", " ", "/\\\u0000", ".", "..", "../")) {
      assertEquals("file", FileNameRules.sanitize(name))
    }
    assertEquals("file (2)", FileNameRules.unique("", setOf("file")))
  }
}
