package ai.gxlself.remotescreen.logic.terminal

import java.io.ByteArrayOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TermSwiftParityTest {
    @Test
    fun modifierApplyMatchesSwiftCases() {
        assertBytes(0x03, actual = TermModifierApply.apply(bytes("c"), 1).bytes)
        assertBytes(0x03, actual = TermModifierApply.apply(bytes("C"), 1).bytes)
        assertBytes(0x1b, actual = TermModifierApply.apply(bytes("["), 1).bytes)
        assertBytes(0x7f, actual = TermModifierApply.apply(bytes("?"), 1).bytes)
        assertBytes(0x1b, 0x62, actual = TermModifierApply.apply(bytes("b"), 2).bytes)
        assertBytes(0x1b, 0x03, actual = TermModifierApply.apply(bytes("c"), 3).bytes)
        assertBytes(0x63, actual = TermModifierApply.apply(bytes("c"), 0).bytes)
        assertFalse(TermModifierApply.apply(bytes("c"), 0).consumed)
        assertBytes(0x1b, 0x5b, 0x41, actual = TermModifierApply.apply(bytes("\u001b[A"), 3).bytes)
        assertFalse(TermModifierApply.apply(bytes("\u001b[A"), 3).consumed)

        mapOf("@" to 0x00, " " to 0x00, "\\" to 0x1c, "]" to 0x1d, "^" to 0x1e, "_" to 0x1f)
            .forEach { (key, expected) ->
                val applied = TermModifierApply.apply(bytes(key), 1)
                assertBytes(expected, actual = applied.bytes)
                assertTrue(key, applied.consumed)
            }
        assertFalse(TermModifierApply.apply(bytes("1"), 1).consumed)
        assertArrayEquals(byteArrayOf(0x1b) + bytes("é"), TermModifierApply.apply(bytes("é"), 2).bytes)
        assertFalse(TermModifierApply.apply(bytes("ab"), 2).consumed)
        assertFalse(TermModifierApply.apply(byteArrayOf(0xff.toByte()), 2).consumed)
    }

    @Test
    fun keySequencesMatchSwiftCases() {
        assertBytes(0x1b, 0x5b, 0x41, actual = TermKeySequences.bytes("up", false, 0))
        assertBytes(0x1b, 0x4f, 0x41, actual = TermKeySequences.bytes("up", true, 0))
        assertBytes(0x1b, 0x5b, 0x31, 0x3b, 0x35, 0x43, actual = TermKeySequences.bytes("right", false, 1))
        assertBytes(0x1b, 0x5b, 0x31, 0x3b, 0x33, 0x44, actual = TermKeySequences.bytes("left", false, 2))
        assertBytes(0x1b, actual = TermKeySequences.bytes("esc", false, 0))
        assertBytes(0x1b, 0x1b, actual = TermKeySequences.bytes("esc", false, 2))
        assertBytes(0x09, actual = TermKeySequences.bytes("tab", false, 0))
        assertArrayEquals(bytes("|"), TermKeySequences.bytes("|", false, 0))
        assertArrayEquals(bytes("\u001b[H"), TermKeySequences.bytes("home", false, 0))
        assertArrayEquals(bytes("\u001bOF"), TermKeySequences.bytes("end", true, 0))
        assertArrayEquals(bytes("\u001b[1;7B"), TermKeySequences.bytes("down", true, 3))
        assertArrayEquals(bytes("\u001b[5~"), TermKeySequences.bytes("pageUp", false, 0))
        assertArrayEquals(bytes("\u001b[6;5~"), TermKeySequences.bytes("pageDown", false, 1))
        assertArrayEquals(byteArrayOf(0x1b) + bytes("é"), TermKeySequences.bytes("é", false, 2))
        assertTrue(TermKeySequences.bytes("unknown", false, 3).isEmpty())
    }

    @Test
    fun pasteEncoderMatchesSwiftCases() {
        assertArrayEquals(bytes("a\rb"), TermPasteEncoder.encode("a\nb", false))
        assertArrayEquals(bytes("\u001b[200~a\rb\u001b[201~"), TermPasteEncoder.encode("a\nb", true))
        assertArrayEquals(bytes("x[201~y"), TermPasteEncoder.encode("x\u001b[201~y", false))
        assertArrayEquals(bytes("a\rb\rc\rd"), TermPasteEncoder.encode("a\r\nb\rc\nd", false))
        assertArrayEquals(
            bytes("\u001b[200~[201~\u001b[201~"),
            TermPasteEncoder.encode("\u001b[201~\u001b", true),
        )
    }

    @Test
    fun outputCoalescerMatchesSwiftCases() {
        val coalescer = TermOutputCoalescer()
        coalescer.append(ByteArray(100 * 1024) { 0x41 })
        coalescer.append(ByteArray(100 * 1024) { 0x42 })
        coalescer.append(ByteArray(100 * 1024) { 0x43 })
        assertEquals(300 * 1024, coalescer.pendingBytes)
        assertArrayEquals(
            ByteArray(100 * 1024) { 0x41 } +
                ByteArray(100 * 1024) { 0x42 } +
                ByteArray(56 * 1024) { 0x43 },
            coalescer.take(256 * 1024),
        )
        assertEquals(44 * 1024, coalescer.pendingBytes)
        assertArrayEquals(ByteArray(44 * 1024) { 0x43 }, coalescer.take(256 * 1024))
        assertNull(coalescer.take(256 * 1024))
        assertEquals(0, coalescer.pendingBytes)

        coalescer.append(byteArrayOf(2, 3, 4))
        assertNull(coalescer.take(0))
        assertBytes(2, 3, actual = requireNotNull(coalescer.take(2)))
        assertBytes(4, actual = requireNotNull(coalescer.take(2)))
    }

    @Test
    fun outputCoalescerSupportsConcurrentProducersAndConsumer() {
        val coalescer = TermOutputCoalescer()
        val start = CountDownLatch(1)
        val producersDone = CountDownLatch(4)
        val executor = Executors.newFixedThreadPool(5)
        repeat(4) { producer ->
            executor.submit {
                start.await()
                repeat(1000) { index ->
                    coalescer.append(
                        byteArrayOf(producer.toByte(), (index shr 8).toByte(), index.toByte()),
                    )
                }
                producersDone.countDown()
            }
        }
        val consumed =
            executor.submit<ByteArray> {
                val output = ByteArrayOutputStream()
                start.await()
                while (producersDone.count > 0 || coalescer.pendingBytes > 0) {
                    val chunk = coalescer.take(4096)
                    if (chunk == null) Thread.yield() else output.write(chunk)
                }
                output.toByteArray()
            }

        start.countDown()
        assertTrue(producersDone.await(10, TimeUnit.SECONDS))
        val allBytes = consumed.get(10, TimeUnit.SECONDS)
        executor.shutdown()
        assertEquals(12_000, allBytes.size)
        val next = IntArray(4)
        allBytes.indices.step(3).forEach { offset ->
            val producer = allBytes[offset].toInt()
            val index =
                ((allBytes[offset + 1].toInt() and 0xff) shl 8) or
                    (allBytes[offset + 2].toInt() and 0xff)
            assertEquals(next[producer], index)
            next[producer]++
        }
        assertArrayEquals(intArrayOf(1000, 1000, 1000, 1000), next)
        assertNull(coalescer.take(1))
    }

    @Test
    fun fontSizeClampMatchesSwiftCases() {
        assertEquals(9.0, TermFontSize.clamp(4.0), 0.0)
        assertEquals(24.0, TermFontSize.clamp(40.0), 0.0)
        assertEquals(13.5, TermFontSize.clamp(13.5), 0.0)
        assertEquals(13.0, TermFontSize.clamp(Double.NaN), 0.0)
    }

    @Test
    fun nightPaletteMatchesSwiftCases() {
        val theme = TermTheme.NIGHT
        assertEquals(16, theme.ansi.size)
        assertEquals(TermRgb(7, 9, 11), theme.background)
        assertEquals(TermRgb(213, 220, 224), theme.foreground)
        assertEquals(TermRgb(236, 240, 242), theme.cursor)
        assertEquals(TermRgb(93, 211, 154), theme.ansi[2])
        assertEquals(TermRgb(134, 146, 154), theme.ansi[8])
    }

    private fun bytes(value: String): ByteArray = value.toByteArray(Charsets.UTF_8)

    private fun assertBytes(vararg expected: Int, actual: ByteArray) {
        assertArrayEquals(expected.map(Int::toByte).toByteArray(), actual)
    }
}
