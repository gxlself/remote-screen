package ai.gxlself.remotescreen.logic.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SessionShortcutsTest {
    // KeyEvent meta states: Ctrl / Alt / Meta on the left side, and the right side.
    private val leftMeta = 0x1000 or 0x2000 or 0x02 or 0x10 or 0x10000 or 0x20000
    private val rightMeta = 0x1000 or 0x4000 or 0x02 or 0x20 or 0x10000 or 0x40000

    @Test
    fun arrowsGoToTheNextAndPreviousSession() {
        assertEquals(SessionShortcut("next"), SessionShortcuts.forKeyEvent(KEYCODE_DPAD_RIGHT, leftMeta))
        assertEquals(SessionShortcut("previous"), SessionShortcuts.forKeyEvent(KEYCODE_DPAD_LEFT, leftMeta))
        assertEquals(SessionShortcut("next"), SessionShortcuts.forKeyEvent(KEYCODE_DPAD_RIGHT, rightMeta))
    }

    @Test
    fun digitsOneToNinePickASession() {
        for (digit in 1..9) {
            assertEquals(
                SessionShortcut("index", digit),
                SessionShortcuts.forKeyEvent(KEYCODE_1 + digit - 1, leftMeta),
            )
        }
        assertNull(SessionShortcuts.forKeyEvent(KEYCODE_0, leftMeta))
    }

    @Test
    fun sOpensTheSwitcher() {
        assertEquals(SessionShortcut("switcher"), SessionShortcuts.forKeyEvent(KEYCODE_S, leftMeta))
    }

    @Test
    fun otherModifierSetsReachTheComputer() {
        val shift = 0x01 or 0x40
        val others = listOf(
            0,
            0x1000 or 0x2000,
            0x10000 or 0x20000,
            0x02 or 0x10 or 0x10000 or 0x20000,
            leftMeta or shift,
        )
        for (meta in others) {
            assertNull(SessionShortcuts.forKeyEvent(KEYCODE_DPAD_RIGHT, meta))
            assertNull(SessionShortcuts.forKeyEvent(KEYCODE_1, meta))
            assertNull(SessionShortcuts.forKeyEvent(KEYCODE_S, meta))
        }
    }

    @Test
    fun otherKeysReachTheComputer() {
        for (keyCode in listOf(KEYCODE_A, KEYCODE_DPAD_UP, KEYCODE_DPAD_DOWN, 66, 111)) {
            assertNull(SessionShortcuts.forKeyEvent(keyCode, leftMeta))
        }
    }

    @Test
    fun matchesTheWireMaskLikeIos() {
        assertEquals(SessionShortcut("next"), SessionShortcuts.match(0x4F, 0x0D))
        assertEquals(SessionShortcut("index", 9), SessionShortcuts.match(0x26, 0xD0))
        assertNull(SessionShortcuts.match(0x4F, 0x0F))
        assertNull(SessionShortcuts.match(0x4F, 0x08))
    }

    private companion object {
        const val KEYCODE_0 = 7
        const val KEYCODE_1 = 8
        const val KEYCODE_A = 29
        const val KEYCODE_S = 47
        const val KEYCODE_DPAD_UP = 19
        const val KEYCODE_DPAD_DOWN = 20
        const val KEYCODE_DPAD_LEFT = 21
        const val KEYCODE_DPAD_RIGHT = 22
    }
}
