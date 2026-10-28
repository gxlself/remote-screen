package ai.gxlself.remotescreen.logic.files

import java.io.IOException
import java.io.InputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class InboxCopyTest {
  @get:Rule val temporary = TemporaryFolder()

  @Test
  fun copiesWithoutOverwritingAnExistingName() {
    val directory = temporary.newFolder()
    val first = InboxCopy.copy(directory, "a.txt") { "first".byteInputStream() }
    val second = InboxCopy.copy(directory, "a.txt") { "second".byteInputStream() }
    assertEquals("a (2).txt", second.name)
    assertArrayEquals("first".toByteArray(), first.readBytes())
    assertArrayEquals("second".toByteArray(), second.readBytes())
  }

  @Test
  fun removesPartialCopyWhenReadingFails() {
    val directory = temporary.newFolder()
    val source = object : InputStream() {
      var delivered = false
      override fun read(): Int = throw IOException("Failed source")
      override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
        if (delivered) throw IOException("Failed source")
        buffer[offset] = 1
        delivered = true
        return 1
      }
    }
    try {
      InboxCopy.copy(directory, "partial") { source }
      fail("Expected failed copy")
    } catch (_: IOException) {
      assertTrue(directory.list().orEmpty().isEmpty())
    }
  }

  @Test
  fun removesReservedNameWhenOpeningFails() {
    val directory = temporary.newFolder()
    try {
      InboxCopy.copy(directory, "partial") { throw IOException("Cannot open") }
      fail("Expected failed open")
    } catch (_: IOException) {
      assertTrue(directory.list().orEmpty().isEmpty())
    }
  }

  @Test
  fun streamsWithABoundedBuffer() {
    val directory = temporary.newFolder()
    var remaining = 4 * 1024 * 1024
    var largestRead = 0
    var closed = false
    val source = object : InputStream() {
      override fun read(): Int = error("Expected buffered read")
      override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
        largestRead = maxOf(largestRead, length)
        if (remaining == 0) return -1
        val count = minOf(length, remaining)
        buffer.fill(0, offset, offset + count)
        remaining -= count
        return count
      }
      override fun close() { closed = true }
    }
    val result = InboxCopy.copy(directory, "large") { source }
    assertEquals(4 * 1024 * 1024L, result.length())
    assertTrue(largestRead <= 64 * 1024)
    assertTrue(closed)
  }
}
