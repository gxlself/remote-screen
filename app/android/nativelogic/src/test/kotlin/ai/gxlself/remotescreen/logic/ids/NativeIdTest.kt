package ai.gxlself.remotescreen.logic.ids

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NativeIdTest {
    @Test
    fun parsesUnsignedDecimalIncludingHighBit() {
        assertEquals(0L, NativeId.parse("0"))
        assertEquals(42L, NativeId.parse("42"))
        assertEquals(-1L, NativeId.parse("18446744073709551615"))
        assertEquals(Long.MIN_VALUE, NativeId.parse("9223372036854775808"))
        assertEquals(1L, NativeId.parse("00000000000000000001"))
    }

    @Test
    fun rejectsOverflowAndNonAsciiDecimal() {
        for (value in listOf(
            "18446744073709551616", "", "-1", " 1", "1a", "100000000000000000000", "٣",
            "+1", "1 ", "1\n", "１",
        )) {
            assertNull(value, NativeId.parse(value))
        }
    }

    @Test
    fun formatsUnsignedMaximum() {
        assertEquals("18446744073709551615", NativeId.format(-1L))
    }

    @Test
    fun roundTripsFiveBitPatterns() {
        for (id in listOf(0L, 42L, Long.MAX_VALUE, Long.MIN_VALUE, -1L)) {
            assertEquals(id, NativeId.parse(NativeId.format(id)))
        }
    }
}
