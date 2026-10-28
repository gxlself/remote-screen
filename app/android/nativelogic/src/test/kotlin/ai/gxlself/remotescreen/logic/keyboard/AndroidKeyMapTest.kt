package ai.gxlself.remotescreen.logic.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidKeyMapTest {
    @Test
    fun mapsLetterAndNumberBoundaries() {
        assertUsages(
            29 to 0x04,
            54 to 0x1D,
            8 to 0x1E,
            16 to 0x26,
            7 to 0x27,
        )
    }

    @Test
    fun mapsEditingAndAnsiPunctuationKeys() {
        assertUsages(
            66 to 0x28,
            111 to 0x29,
            67 to 0x2A,
            61 to 0x2B,
            62 to 0x2C,
            69 to 0x2D,
            70 to 0x2E,
            71 to 0x2F,
            72 to 0x30,
            73 to 0x31,
            74 to 0x33,
            75 to 0x34,
            68 to 0x35,
            55 to 0x36,
            56 to 0x37,
            76 to 0x38,
            115 to 0x39,
        )
    }

    @Test
    fun mapsFunctionKeyBoundaries() {
        assertUsages(131 to 0x3A, 135 to 0x3E, 142 to 0x45)
    }

    @Test
    fun mapsNavigationAndLockKeys() {
        assertUsages(
            120 to 0x46,
            116 to 0x47,
            121 to 0x48,
            124 to 0x49,
            122 to 0x4A,
            92 to 0x4B,
            112 to 0x4C,
            123 to 0x4D,
            93 to 0x4E,
        )
    }

    @Test
    fun mapsAllFourArrowKeys() {
        assertUsages(22 to 0x4F, 21 to 0x50, 20 to 0x51, 19 to 0x52)
    }

    @Test
    fun mapsNumpadBoundariesAndOperators() {
        assertUsages(
            143 to 0x53,
            154 to 0x54,
            155 to 0x55,
            156 to 0x56,
            157 to 0x57,
            160 to 0x58,
            145 to 0x59,
            153 to 0x61,
            144 to 0x62,
            158 to 0x63,
        )
    }

    @Test
    fun mapsMenuAndAllModifierUsages() {
        assertUsages(
            82 to 0x65,
            113 to 0xE0,
            59 to 0xE1,
            57 to 0xE2,
            117 to 0xE3,
            114 to 0xE4,
            60 to 0xE5,
            58 to 0xE6,
            118 to 0xE7,
        )
        listOf(113, 59, 57, 117, 114, 60, 58, 118).forEach {
            assertTrue(AndroidKeyMap.isModifier(it))
        }
        assertFalse(AndroidKeyMap.isModifier(29))
    }

    @Test
    fun leavesAndroidSystemKeysUnmapped() {
        assertNull(AndroidKeyMap.usage(24)) // KeyEvent.KEYCODE_VOLUME_UP
        assertNull(AndroidKeyMap.usage(4)) // KeyEvent.KEYCODE_BACK
        assertNull(AndroidKeyMap.usage(3)) // KeyEvent.KEYCODE_HOME
        assertNull(AndroidKeyMap.usage(Int.MAX_VALUE))
    }

    @Test
    fun mapsExplicitLeftAndRightMetaStateBits() {
        val everySide = 0x2000 or 0x4000 or 0x40 or 0x80 or 0x10 or 0x20 or 0x20000 or 0x40000
        assertEquals(0xFF, AndroidKeyMap.mods(everySide))
        assertEquals(0x01, AndroidKeyMap.mods(0x2000))
        assertEquals(0x10, AndroidKeyMap.mods(0x4000))
        assertEquals(0x02, AndroidKeyMap.mods(0x40))
        assertEquals(0x20, AndroidKeyMap.mods(0x80))
        assertEquals(0x04, AndroidKeyMap.mods(0x10))
        assertEquals(0x40, AndroidKeyMap.mods(0x20))
        assertEquals(0x08, AndroidKeyMap.mods(0x20000))
        assertEquals(0x80, AndroidKeyMap.mods(0x40000))
    }

    @Test
    fun genericMetaStateBitsFallBackToLeftSide() {
        assertEquals(0x0F, AndroidKeyMap.mods(0x1 or 0x2 or 0x1000 or 0x10000))
    }

    @Test
    fun explicitSideWinsOverGenericFallback() {
        assertEquals(0x20, AndroidKeyMap.mods(0x1 or 0x80))
        assertEquals(0x10, AndroidKeyMap.mods(0x1000 or 0x4000))
    }

    private fun assertUsages(vararg mappings: Pair<Int, Int>) {
        mappings.forEach { (keyCode, usage) -> assertEquals(usage, AndroidKeyMap.usage(keyCode)) }
    }
}
