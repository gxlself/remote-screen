package ai.gxlself.remotescreen.logic.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HardwareKeyPlannerTest {
    @Test
    fun ctrlCTracksModifierStateAcrossFourEvents() {
        val events =
            listOf(
                plan(113, down = true, metaState = 0x1000 or 0x2000),
                plan(31, down = true, metaState = 0x1000 or 0x2000),
                plan(31, down = false, metaState = 0x1000 or 0x2000),
                plan(113, down = false, metaState = 0),
            )
        assertEquals(listOf(0x01, 0x01, 0x01, 0x00), events.map(::onlyKey).map(KeyAction.Key::mods))
        assertEquals(listOf(0xE0, 0x06, 0x06, 0xE0), events.map(::onlyKey).map(KeyAction.Key::usage))
        assertEquals(listOf(true, true, false, false), events.map(::onlyKey).map(KeyAction.Key::down))
    }

    @Test
    fun rightShiftAUsesRightModifierBit() {
        assertEquals(
            KeyAction.Key(0xE5, down = true, mods = 0x20),
            onlyKey(plan(60, down = true, metaState = 0x1 or 0x80)),
        )
        assertEquals(
            KeyAction.Key(0x04, down = true, mods = 0x20),
            onlyKey(plan(29, down = true, metaState = 0x1 or 0x80)),
        )
    }

    @Test
    fun windowsMapsLeftMetaShortcutToLeftControl() {
        val metaDown = plan(117, down = true, metaState = 0x10000 or 0x20000, os = KeyboardOs.WINDOWS)
        val aDown = plan(29, down = true, metaState = 0x10000 or 0x20000, os = KeyboardOs.WINDOWS)
        assertEquals(KeyAction.Key(0xE0, down = true, mods = 0x01), onlyKey(metaDown))
        assertEquals(KeyAction.Key(0x04, down = true, mods = 0x01), onlyKey(aDown))
    }

    @Test
    fun stickyModifierIsConsumedOnDownAndAbsentFromUp() {
        assertEquals(
            listOf(
                KeyAction.Key(0x06, down = true, mods = 0x08),
                KeyAction.StickyConsumed(0x08),
            ),
            plan(31, down = true, stickyMods = 0x08),
        )
        assertEquals(
            listOf(KeyAction.Key(0x06, down = false, mods = 0)),
            plan(31, down = false, stickyMods = 0x08),
        )
    }

    @Test
    fun unknownVolumeKeyReturnsNull() {
        assertNull(HardwareKeyPlanner.plan(24, true, 0, KeyboardOs.MACOS, 0))
    }

    @Test
    fun plansF5() {
        assertEquals(KeyAction.Key(0x3E, down = true, mods = 0), onlyKey(plan(135, down = true)))
    }

    @Test
    fun plansNumpadEnter() {
        assertEquals(KeyAction.Key(0x58, down = true, mods = 0), onlyKey(plan(160, down = true)))
    }

    @Test
    fun plansArrowKey() {
        assertEquals(KeyAction.Key(0x50, down = false, mods = 0), onlyKey(plan(21, down = false)))
    }

    private fun plan(
        keyCode: Int,
        down: Boolean,
        metaState: Int = 0,
        os: KeyboardOs = KeyboardOs.MACOS,
        stickyMods: Int = 0,
    ): List<KeyAction> =
        requireNotNull(HardwareKeyPlanner.plan(keyCode, down, metaState, os, stickyMods))

    private fun onlyKey(actions: List<KeyAction>): KeyAction.Key {
        assertEquals(1, actions.size)
        return actions.single() as KeyAction.Key
    }
}
