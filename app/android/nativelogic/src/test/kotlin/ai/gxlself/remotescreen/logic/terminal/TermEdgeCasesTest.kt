package ai.gxlself.remotescreen.logic.terminal

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class TermEdgeCasesTest {
    @Test
    fun modifierUsesOneUnicodeCodePointRatherThanOneGraphemeCluster() {
        val combiningGrapheme = "e\u0301".toByteArray(Charsets.UTF_8)
        val result = TermModifierApply.apply(combiningGrapheme, TermMods.ALT)
        assertArrayEquals(combiningGrapheme, result.bytes)
        assertFalse(result.consumed)
    }

    @Test
    fun modifierAcceptsOneSupplementaryCodePoint() {
        val emoji = "😀".toByteArray(Charsets.UTF_8)
        val result = TermModifierApply.apply(emoji, TermMods.ALT)
        assertArrayEquals(byteArrayOf(0x1b) + emoji, result.bytes)
        assertTrue(result.consumed)
    }

    @Test
    fun appendCopiesMutableInput() {
        val source = byteArrayOf(1, 2, 3)
        val coalescer = TermOutputCoalescer()
        coalescer.append(source)
        source[0] = 9
        assertArrayEquals(byteArrayOf(1, 2, 3), coalescer.take(3))
    }

    @Test
    fun rgbParsesHexAndFallsBackToBlack() {
        assertEquals(TermRgb(213, 220, 224), TermTheme.rgb("  ##D5DCE0 \n"))
        assertEquals(TermRgb(0, 0, 0), TermTheme.rgb("bad"))
        assertEquals(TermRgb(0, 0, 0), TermTheme.rgb("#GG0000"))
    }

    @Test
    fun rgbConvertsToOpaqueArgb() {
        assertEquals(0xff123456L.toInt(), TermRgb(0x12, 0x34, 0x56).argb())
    }

    @Test
    fun namedThemeDefaultsToNight() {
        assertSame(TermTheme.NIGHT, TermTheme.named("night"))
        assertSame(TermTheme.NIGHT, TermTheme.named("unknown"))
    }

    @Test
    fun fontSizeNonFiniteValuesUseDefault() {
        assertEquals(13.0, TermFontSize.clamp(Double.POSITIVE_INFINITY), 0.0)
        assertEquals(13.0, TermFontSize.clamp(Double.NEGATIVE_INFINITY), 0.0)
    }
}
