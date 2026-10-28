package ai.gxlself.remotescreen.logic.keyboard

/**
 * A Control + Alt + Meta (⌃⌥⌘ on a Mac keyboard) session shortcut.
 * It stays on this device: the capture view sends it to JS, not to the computer.
 * [action] is next / previous / index / switcher; [index] is 1..9 for index.
 */
data class SessionShortcut(val action: String, val index: Int = 0)

object SessionShortcuts {
    /** Control + Option + Command in the wire HID mask, either side. */
    private const val CONTROL_OPTION_COMMAND = 0x0D

    /** [usage] and [mods] as sent to the computer (wire HID). */
    fun match(usage: Int, mods: Int): SessionShortcut? {
        val normalized = (mods and 0x0F) or ((mods shr 4) and 0x0F)
        if (normalized != CONTROL_OPTION_COMMAND) return null
        return when (usage) {
            0x4F -> SessionShortcut("next")
            0x50 -> SessionShortcut("previous")
            in 0x1E..0x26 -> SessionShortcut("index", usage - 0x1E + 1)
            0x16 -> SessionShortcut("switcher")
            else -> null
        }
    }

    /** An Android key event: KeyEvent.getKeyCode() and getMetaState(). */
    fun forKeyEvent(keyCode: Int, metaState: Int): SessionShortcut? {
        val usage = AndroidKeyMap.usage(keyCode) ?: return null
        return match(usage, AndroidKeyMap.mods(metaState))
    }
}
