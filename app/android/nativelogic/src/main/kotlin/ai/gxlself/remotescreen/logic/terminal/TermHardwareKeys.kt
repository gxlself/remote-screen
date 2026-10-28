package ai.gxlself.remotescreen.logic.terminal

object TermHardwareKeys {
    fun bytes(keyCode: Int, appCursor: Boolean, mods: Int): ByteArray? =
        when (keyCode) {
            KEYCODE_DPAD_UP -> TermKeySequences.bytes("up", appCursor, mods)
            KEYCODE_DPAD_DOWN -> TermKeySequences.bytes("down", appCursor, mods)
            KEYCODE_DPAD_RIGHT -> TermKeySequences.bytes("right", appCursor, mods)
            KEYCODE_DPAD_LEFT -> TermKeySequences.bytes("left", appCursor, mods)
            KEYCODE_MOVE_HOME -> TermKeySequences.bytes("home", appCursor, mods)
            KEYCODE_MOVE_END -> TermKeySequences.bytes("end", appCursor, mods)
            KEYCODE_PAGE_UP -> TermKeySequences.bytes("pageUp", appCursor, mods)
            KEYCODE_PAGE_DOWN -> TermKeySequences.bytes("pageDown", appCursor, mods)
            KEYCODE_ESCAPE -> TermKeySequences.bytes("esc", appCursor, mods)
            KEYCODE_TAB -> TermKeySequences.bytes("tab", appCursor, mods)
            KEYCODE_ENTER, KEYCODE_NUMPAD_ENTER ->
                TermModifierApply.apply(byteArrayOf(CARRIAGE_RETURN), mods).bytes
            KEYCODE_DEL -> TermModifierApply.apply(byteArrayOf(DELETE), mods).bytes
            KEYCODE_FORWARD_DEL -> TermKeySequences.tilde("3", mods)
            KEYCODE_INSERT -> TermKeySequences.tilde("2", mods)
            in KEYCODE_F1..KEYCODE_F4 -> functionKeyOneToFour(keyCode, mods)
            in KEYCODE_F5..KEYCODE_F12 -> functionKeyFiveToTwelve(keyCode, mods)
            else -> null
        }

    private fun functionKeyOneToFour(keyCode: Int, mods: Int): ByteArray {
        val final = 'P' + (keyCode - KEYCODE_F1)
        val modifier = TermKeySequences.xtermModifier(mods)
        val sequence =
            if (modifier == 1) "\u001bO$final" else "\u001b[1;$modifier$final"
        return sequence.toByteArray(Charsets.UTF_8)
    }

    private fun functionKeyFiveToTwelve(keyCode: Int, mods: Int): ByteArray {
        val prefixes = intArrayOf(15, 17, 18, 19, 20, 21, 23, 24)
        return TermKeySequences.tilde(prefixes[keyCode - KEYCODE_F5].toString(), mods)
    }

    // Android KeyEvent key-code values. Kept local so this pure-JVM module has no android.* dependency.
    private const val KEYCODE_DPAD_UP = 19
    private const val KEYCODE_DPAD_DOWN = 20
    private const val KEYCODE_DPAD_LEFT = 21
    private const val KEYCODE_DPAD_RIGHT = 22
    private const val KEYCODE_TAB = 61
    private const val KEYCODE_ENTER = 66
    private const val KEYCODE_DEL = 67
    private const val KEYCODE_PAGE_UP = 92
    private const val KEYCODE_PAGE_DOWN = 93
    private const val KEYCODE_ESCAPE = 111
    private const val KEYCODE_FORWARD_DEL = 112
    private const val KEYCODE_MOVE_HOME = 122
    private const val KEYCODE_MOVE_END = 123
    private const val KEYCODE_INSERT = 124
    private const val KEYCODE_F1 = 131
    private const val KEYCODE_F4 = 134
    private const val KEYCODE_F5 = 135
    private const val KEYCODE_F12 = 142
    private const val KEYCODE_NUMPAD_ENTER = 160

    private const val CARRIAGE_RETURN: Byte = 0x0d
    private const val DELETE: Byte = 0x7f
}

object TermMods {
    const val CTRL = 1
    const val ALT = 2

    fun fromAndroidMeta(metaState: Int): Int {
        var mods = 0
        // Android KeyEvent.META_CTRL_ON.
        if (metaState and 0x1000 != 0) mods = mods or CTRL
        // Android KeyEvent.META_ALT_ON.
        if (metaState and 0x0002 != 0) mods = mods or ALT
        return mods
    }
}
