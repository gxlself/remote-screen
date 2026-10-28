package ai.gxlself.remotescreen.logic.crash

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeCrashInboxTest {
  @Test
  fun selectFiltersReasonsAndSorts() {
    val records = listOf(
      rec(6, 1_000),
      rec(10, 5_000),
      rec(4, 3_000),
      rec(3, 2_000),
      rec(5, 4_000),
    )

    val selection = NativeCrashInbox.select(records, null, 10_000)

    assertEquals(listOf(1_000L, 3_000L, 4_000L), selection.records.map { it.timestampMs })
    assertEquals(listOf(6, 4, 5), selection.records.map { it.reason })
    assertEquals(5_000L, selection.lastHandledMs)
  }

  @Test
  fun selectSkipsHandled() {
    val records = listOf(
      rec(6, 1_000),
      rec(10, 5_000),
      rec(4, 3_000),
      rec(3, 2_000),
      rec(5, 4_000),
    )

    val selection = NativeCrashInbox.select(records, 3_000, 10_000)

    assertEquals(listOf(4_000L), selection.records.map { it.timestampMs })
    assertEquals(5_000L, selection.lastHandledMs)
  }

  @Test
  fun selectSkipsOld() {
    val now = NativeCrashInbox.MAX_AGE_MS + 2_000
    val selection = NativeCrashInbox.select(
      listOf(rec(4, 1_000), rec(4, now - 1_000)),
      null,
      now,
    )

    assertEquals(listOf(now - 1_000), selection.records.map { it.timestampMs })
  }

  @Test
  fun selectKeepsNewestFive() {
    val records = (1L..8L).map { timestamp -> rec(4, timestamp) }

    val selection = NativeCrashInbox.select(records, null, 8)

    assertEquals(listOf(4L, 5L, 6L, 7L, 8L), selection.records.map { it.timestampMs })
    assertEquals(8L, selection.lastHandledMs)
  }

  @Test
  fun selectEmptyKeepsLast() {
    assertEquals(
      ExitSelection(emptyList(), 77L),
      NativeCrashInbox.select(emptyList(), 77L, 100),
    )
    assertEquals(
      ExitSelection(emptyList(), null),
      NativeCrashInbox.select(emptyList(), null, 100),
    )
    assertEquals(
      ExitSelection(emptyList(), 77L),
      NativeCrashInbox.select(listOf(rec(4, 50)), 77L, 100),
    )
  }

  @Test
  fun selectIgnoresZeroTimestamp() {
    val selection = NativeCrashInbox.select(listOf(rec(4, 0)), null, 100)

    assertTrue(selection.records.isEmpty())
    assertEquals(0L, selection.lastHandledMs)
  }

  @Test
  fun names() {
    assertEquals("JAVA_CRASH", NativeCrashInbox.name(rec(4, 1)))
    assertEquals("NATIVE_CRASH SIGSEGV", NativeCrashInbox.name(rec(5, 1, status = 11)))
    assertEquals("NATIVE_CRASH SIGBUS", NativeCrashInbox.name(rec(5, 1, status = 7)))
    assertEquals("NATIVE_CRASH", NativeCrashInbox.name(rec(5, 1)))
    assertEquals("NATIVE_CRASH SIG99", NativeCrashInbox.name(rec(5, 1, status = 99)))
    assertEquals("ANR", NativeCrashInbox.name(rec(6, 1, status = 3)))
    assertEquals("EXIT_13", NativeCrashInbox.name(rec(13, 1)))
  }

  @Test
  fun toInboxExact() {
    val inbox = NativeCrashInbox.toInbox(
      rec(
        5,
        1_792_000_000_000L,
        pid = 1_234,
        status = 11,
        description = "  Fatal signal 11  ",
      ),
      "0.1.0",
      "34",
    )

    assertEquals("00001792000000000000-exitinfo-1234.json", inbox.name)
    assertEquals(
      """{"v":1,"incidentAt":1792000000000000,"appVersion":"0.1.0","osVersion":"34","name":"NATIVE_CRASH SIGSEGV","message":"Fatal signal 11","stack":null,"thread":"ai.gxlself.remotescreen"}""",
      inbox.json,
    )
  }

  @Test
  fun toInboxAnrTrace() {
    val trace = "\"main\" prio=5\n  at a.b(C.java:1)\n\tx\u0001"
    val anr = NativeCrashInbox.toInbox(rec(6, 5, trace = trace), null, null)
    val expectedStack = "\\\"main\\\" prio=5\\n  at a.b(C.java:1)\\n\\tx\\u0001"

    assertTrue(anr.json.contains("\"message\":\"ANR\""))
    assertTrue(anr.json.contains("\"stack\":\"$expectedStack\""))
    assertTrue(
      NativeCrashInbox.toInbox(rec(4, 5, trace = trace), null, null)
        .json.contains("\"stack\":null"),
    )
  }

  @Test
  fun traceTruncation() {
    val longTrace = NativeCrashInbox.toInbox(
      rec(6, 1, trace = "a".repeat(20_000)),
      null,
      null,
    )
    val stack = longTrace.json.substringAfter("\"stack\":\"").substringBefore("\",\"thread\"")
    assertEquals(16_384, stack.length)

    val surrogateTrace = "a".repeat(16_383) + "😀"
    val surrogateInbox = NativeCrashInbox.toInbox(rec(6, 1, trace = surrogateTrace), null, null)
    val surrogateStack = surrogateInbox.json
      .substringAfter("\"stack\":\"")
      .substringBefore("\",\"thread\"")
    assertEquals(16_383, surrogateStack.length)
    assertFalse(surrogateStack.last().isHighSurrogate())
  }

  @Test
  fun emptyStrings() {
    val inbox = NativeCrashInbox.toInbox(
      rec(4, 1, description = "   ", processName = ""),
      "",
      null,
    )

    assertTrue(inbox.json.contains("\"message\":\"JAVA_CRASH\""))
    assertTrue(inbox.json.contains("\"thread\":null"))
    assertTrue(inbox.json.contains("\"appVersion\":null"))
    assertTrue(inbox.json.contains("\"osVersion\":null"))
  }

  @Test
  fun escaping() {
    val inbox = NativeCrashInbox.toInbox(
      rec(4, 1, description = "路径 C:\\x \"q\" a/b"),
      null,
      null,
    )

    assertTrue(inbox.json.contains("\"message\":\"路径 C:\\\\x \\\"q\\\" a/b\""))
  }

  @Test
  fun namesToDelete() {
    val names = listOf(
      "00000000000000000003-exitinfo-1.json",
      ".tmp-a",
      "notes.txt",
      "00000000000000000001-exitinfo-1.json",
      "00000000000000000002-metrickit-0.json",
    )

    assertEquals(
      listOf("00000000000000000001-exitinfo-1.json"),
      NativeCrashInbox.namesToDelete(names, keep = 2),
    )
    assertTrue(NativeCrashInbox.namesToDelete(names).isEmpty())
  }

  @Test
  fun inboxDir() {
    assertEquals(
      File(File(File("/x/core"), "crashes"), "native"),
      NativeCrashInbox.inboxDir(File("/x/core")),
    )
  }

  private fun rec(
    reason: Int,
    timestampMs: Long,
    pid: Int = 1,
    status: Int = 0,
    description: String? = null,
    processName: String? = "ai.gxlself.remotescreen",
    trace: String? = null,
  ): ExitRecord = ExitRecord(
    reason = reason,
    timestampMs = timestampMs,
    pid = pid,
    status = status,
    description = description,
    processName = processName,
    trace = trace,
  )
}
