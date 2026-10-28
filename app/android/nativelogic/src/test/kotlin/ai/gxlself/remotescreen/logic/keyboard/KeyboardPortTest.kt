package ai.gxlself.remotescreen.logic.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeyboardPortTest {
    @Test
    fun hidMapLettersNumbersAndUnmappableText() {
        assertMapping("a", 0x04, false)
        assertMapping("z", 0x1D, false)
        assertMapping("A", 0x04, true)
        assertMapping("Z", 0x1D, true)
        assertMapping("1", 0x1E, false)
        assertMapping("9", 0x26, false)
        assertMapping("0", 0x27, false)
        assertMapping("!", 0x1E, true)
        assertMapping("?", 0x38, true)
        assertMapping(" ", 0x2C, false)
        assertMapping("\n", 0x28, false)
        assertMapping("\t", 0x2B, false)
        assertNull(HidMap.usage("中"))
        assertNull(HidMap.usage("é"))
        assertNull(HidMap.usage("👨‍👩‍👧"))
    }

    @Test
    fun hidMapAnsiPunctuationSkipsNonUsUsage() {
        val plain = "-=[]\\;'`,./"
        val shifted = "_+{}|:\"~<>?"
        val usages = listOf(0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38)
        for (index in usages.indices) {
            assertMapping(plain[index].toString(), usages[index], false)
            assertMapping(shifted[index].toString(), usages[index], true)
        }
        for ((index, character) in "!@#$%^&*()".withIndex()) {
            assertMapping(character.toString(), index + 0x1E, true)
        }
    }

    @Test
    fun modifiersUseAfterTransitionStateAndPreserveBothSides() {
        assertEquals(0x08, Modifiers.after(0xE3, down = true, current = 0))
        assertEquals(0x88, Modifiers.after(0xE7, down = true, current = 0x08))
        assertEquals(0x80, Modifiers.after(0xE3, down = false, current = 0x88))
        for (offset in 0 until 8) {
            val mask = 1 shl offset
            assertEquals(mask, Modifiers.after(0xE0 + offset, down = true, current = 0))
            assertEquals(0xFF and mask.inv(), Modifiers.after(0xE0 + offset, down = false, current = 0xFF))
        }
        assertEquals(0x88, Modifiers.after(0x06, down = false, current = 0x88))
    }

    @Test
    fun osMapRemapsCommandToCorrespondingControlSide() {
        assertEquals(0xE0 to 0x01, OsMap.remap(0xE3, 0x08, KeyboardOs.WINDOWS))
        assertEquals(0xE4 to 0x11, OsMap.remap(0xE7, 0x88, KeyboardOs.WINDOWS))
        assertEquals(0x06 to 0x33, OsMap.remap(0x06, 0xAA, KeyboardOs.WINDOWS))
        assertEquals(0xE3 to 0x88, OsMap.remap(0xE3, 0x88, KeyboardOs.MACOS))
    }

    @Test
    fun splitterRespectsUtf8BytesAndGraphemeBoundaries() {
        val ascii = TextSplitter.split(codePointClusters("a".repeat(5000)))
        assertEquals(listOf(4096, 904), ascii.map(::utf8Size))

        val chinese = TextSplitter.split(codePointClusters("中".repeat(2000)))
        assertEquals(listOf(1365, 635), chinese.map(String::length))
        assertEquals(true, chinese.all { utf8Size(it) <= 4096 })

        val family = "👨‍👩‍👧"
        val clusters = listOf("a".repeat(4090), family, "e\u0301")
        val emoji = TextSplitter.split(clusters)
        assertEquals(listOf("a".repeat(4090), family + "e\u0301"), emoji)
        assertEquals(clusters.joinToString(""), emoji.joinToString(""))
        assertEquals(emptyList<String>(), TextSplitter.split(emptyList()))
    }

    @Test
    fun splitterNeverEmitsOversizeOrEmptyFrames() {
        assertEquals(emptyList<String>(), TextSplitter.split(listOf("a"), maxBytes = 0))
        assertEquals(emptyList<String>(), TextSplitter.split(listOf("a"), maxBytes = -1))
        assertEquals(listOf("a", "b"), TextSplitter.split(listOf("a", "中", "b"), maxBytes = 2))
    }

    @Test
    fun textCommitAndNewlines() {
        assertEquals(listOf(KeyAction.Text("你好")), plan("你好"))
        assertEquals(
            listOf(
                KeyAction.Text("a"),
                KeyAction.Key(0x28, down = true, mods = 0),
                KeyAction.Key(0x28, down = false, mods = 0),
                KeyAction.Text("b"),
            ),
            plan("a\nb"),
        )
        assertEquals(emptyList<KeyAction>(), plan(""))
        assertEquals(listOf(KeyAction.Text("a\tb")), plan("a\tb"))
    }

    @Test
    fun keysCommitUsesShiftAndGroupsUnmappableCharacters() {
        assertEquals(
            listOf(
                KeyAction.Key(0x0B, down = true, mods = 0x02),
                KeyAction.Key(0x0B, down = false, mods = 0x02),
                KeyAction.Key(0x0C, down = true, mods = 0),
                KeyAction.Key(0x0C, down = false, mods = 0),
            ),
            plan("Hi", InputMode.KEYS),
        )
        assertEquals(
            listOf(
                KeyAction.Text("中👨‍👩‍👧"),
                KeyAction.Key(0x04, down = true, mods = 0),
                KeyAction.Key(0x04, down = false, mods = 0),
                KeyAction.Text("文"),
            ),
            planWithClusters(
                committed = "中👨‍👩‍👧a文",
                clusters = listOf("中", "👨‍👩‍👧", "a", "文"),
                mode = InputMode.KEYS,
            ),
        )
    }

    @Test
    fun stickyConsumesExactlyOneMappableFirstCharacter() {
        assertEquals(
            listOf(
                KeyAction.Key(0x06, down = true, mods = 0x08),
                KeyAction.Key(0x06, down = false, mods = 0x08),
                KeyAction.StickyConsumed(0x08),
            ),
            plan("c", stickyMods = 0x08),
        )
        assertEquals(
            listOf(
                KeyAction.Key(0x06, down = true, mods = 0x82),
                KeyAction.Key(0x06, down = false, mods = 0x82),
                KeyAction.StickyConsumed(0x80),
                KeyAction.Text("z"),
            ),
            plan("Cz", stickyMods = 0x80),
        )
        assertEquals(listOf(KeyAction.Text("中")), plan("中", stickyMods = 0x08))
        assertEquals(listOf(KeyAction.Text("中c")), plan("中c", stickyMods = 0x08))
        assertEquals(emptyList<KeyAction>(), plan("", stickyMods = 0x08))
    }

    @Test
    fun plannerSplitsTextAndKeyFallbackAtWireLimit() {
        val text = "中".repeat(2000)
        val expected = TextSplitter.split(codePointClusters(text)).map(KeyAction::Text)
        assertEquals(expected, plan(text))
        assertEquals(expected, plan(text, InputMode.KEYS))
    }

    private fun assertMapping(cluster: String, usage: Int, shift: Boolean) {
        assertEquals(usage to shift, HidMap.usage(cluster))
    }

    private fun plan(
        committed: String,
        mode: InputMode = InputMode.TEXT,
        stickyMods: Int = 0,
    ): List<KeyAction> = CommitPlanner.plan(committed, mode, stickyMods, CODE_POINT_SPLITTER)

    private fun planWithClusters(
        committed: String,
        clusters: List<String>,
        mode: InputMode = InputMode.TEXT,
        stickyMods: Int = 0,
    ): List<KeyAction> =
        CommitPlanner.plan(committed, mode, stickyMods, GraphemeSplitter { clusters })

    private fun codePointClusters(text: String): List<String> =
        text.codePoints().toArray().map { String(Character.toChars(it)) }

    private fun utf8Size(text: String): Int = text.toByteArray(Charsets.UTF_8).size

    private companion object {
        val CODE_POINT_SPLITTER = GraphemeSplitter { text ->
            text.codePoints().toArray().map { String(Character.toChars(it)) }
        }
    }
}
