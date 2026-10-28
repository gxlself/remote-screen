package ai.gxlself.remotescreen.logic.terminal

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TermHardwareKeysTest {
    @Test
    fun mapsAllDpadKeys() {
        assertKey(19, "\u001b[A")
        assertKey(20, "\u001b[B")
        assertKey(22, "\u001b[C")
        assertKey(21, "\u001b[D")
    }

    @Test
    fun mapsDpadInApplicationCursorModeAndWithModifiers() {
        assertKey(19, "\u001bOA", appCursor = true)
        assertKey(20, "\u001b[1;7B", appCursor = true, mods = 3)
        assertKey(22, "\u001b[1;5C", mods = 1)
        assertKey(21, "\u001b[1;3D", mods = 2)
    }

    @Test
    fun mapsNavigationKeys() {
        assertKey(122, "\u001b[H")
        assertKey(123, "\u001b[F")
        assertKey(92, "\u001b[5~")
        assertKey(93, "\u001b[6~")
    }

    @Test
    fun mapsModifiedNavigationKeys() {
        assertKey(122, "\u001b[1;5H", mods = 1)
        assertKey(123, "\u001b[1;3F", mods = 2)
        assertKey(92, "\u001b[5;7~", mods = 3)
        assertKey(93, "\u001b[6;5~", mods = 1)
    }

    @Test
    fun mapsEscapeTabAndEnterKeys() {
        assertKey(111, "\u001b")
        assertKey(61, "\t")
        assertKey(66, "\r")
        assertKey(160, "\r")
        assertKey(111, "\u001b\u001b", mods = 2)
        assertKey(61, "\u001b\t", mods = 2)
        assertKey(66, "\u001b\r", mods = 2)
    }

    @Test
    fun mapsDeleteAndForwardDelete() {
        assertArrayEquals(byteArrayOf(0x7f), TermHardwareKeys.bytes(67, false, 0))
        assertArrayEquals(byteArrayOf(0x1b, 0x7f), TermHardwareKeys.bytes(67, false, 2))
        assertKey(112, "\u001b[3~")
        assertKey(112, "\u001b[3;7~", mods = 3)
    }

    @Test
    fun mapsInsertIncludingModifiers() {
        assertKey(124, "\u001b[2~")
        assertKey(124, "\u001b[2;5~", mods = 1)
    }

    @Test
    fun mapsFunctionKeysOneThroughFour() {
        listOf("\u001bOP", "\u001bOQ", "\u001bOR", "\u001bOS").forEachIndexed { index, expected ->
            assertKey(131 + index, expected)
        }
        listOf('P', 'Q', 'R', 'S').forEachIndexed { index, final ->
            assertKey(131 + index, "\u001b[1;7$final", mods = 3)
        }
    }

    @Test
    fun mapsFunctionKeysFiveThroughTwelve() {
        val prefixes = listOf(15, 17, 18, 19, 20, 21, 23, 24)
        prefixes.forEachIndexed { index, prefix ->
            assertKey(135 + index, "\u001b[$prefix~")
            assertKey(135 + index, "\u001b[$prefix;3~", mods = 2)
        }
    }

    @Test
    fun returnsNullForUnknownKey() {
        assertNull(TermHardwareKeys.bytes(1, false, 0))
    }

    @Test
    fun convertsAndroidMetaBitsAndIgnoresOthers() {
        assertEquals(0, TermMods.fromAndroidMeta(0))
        assertEquals(TermMods.CTRL, TermMods.fromAndroidMeta(0x1000))
        assertEquals(TermMods.ALT, TermMods.fromAndroidMeta(0x0002))
        assertEquals(TermMods.CTRL or TermMods.ALT, TermMods.fromAndroidMeta(0x1002))
        assertEquals(0, TermMods.fromAndroidMeta(0x0001 or 0x0001_0000))
    }

    private fun assertKey(
        keyCode: Int,
        expected: String,
        appCursor: Boolean = false,
        mods: Int = 0,
    ) {
        assertArrayEquals(
            expected.toByteArray(Charsets.UTF_8),
            TermHardwareKeys.bytes(keyCode, appCursor, mods),
        )
    }
}
