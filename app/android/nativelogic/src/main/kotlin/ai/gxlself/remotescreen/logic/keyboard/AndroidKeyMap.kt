package ai.gxlself.remotescreen.logic.keyboard

object AndroidKeyMap {
    fun usage(keyCode: Int): Int? =
        when (keyCode) {
            in KEYCODE_A..KEYCODE_Z -> keyCode - KEYCODE_A + 0x04
            in KEYCODE_1..KEYCODE_9 -> keyCode - KEYCODE_1 + 0x1E
            KEYCODE_0 -> 0x27
            KEYCODE_ENTER -> 0x28
            KEYCODE_ESCAPE -> 0x29
            KEYCODE_DEL -> 0x2A
            KEYCODE_TAB -> 0x2B
            KEYCODE_SPACE -> 0x2C
            KEYCODE_MINUS -> 0x2D
            KEYCODE_EQUALS -> 0x2E
            KEYCODE_LEFT_BRACKET -> 0x2F
            KEYCODE_RIGHT_BRACKET -> 0x30
            KEYCODE_BACKSLASH -> 0x31
            KEYCODE_SEMICOLON -> 0x33
            KEYCODE_APOSTROPHE -> 0x34
            KEYCODE_GRAVE -> 0x35
            KEYCODE_COMMA -> 0x36
            KEYCODE_PERIOD -> 0x37
            KEYCODE_SLASH -> 0x38
            KEYCODE_CAPS_LOCK -> 0x39
            in KEYCODE_F1..KEYCODE_F12 -> keyCode - KEYCODE_F1 + 0x3A
            KEYCODE_SYSRQ -> 0x46
            KEYCODE_SCROLL_LOCK -> 0x47
            KEYCODE_BREAK -> 0x48
            KEYCODE_INSERT -> 0x49
            KEYCODE_MOVE_HOME -> 0x4A
            KEYCODE_PAGE_UP -> 0x4B
            KEYCODE_FORWARD_DEL -> 0x4C
            KEYCODE_MOVE_END -> 0x4D
            KEYCODE_PAGE_DOWN -> 0x4E
            KEYCODE_DPAD_RIGHT -> 0x4F
            KEYCODE_DPAD_LEFT -> 0x50
            KEYCODE_DPAD_DOWN -> 0x51
            KEYCODE_DPAD_UP -> 0x52
            KEYCODE_NUM_LOCK -> 0x53
            KEYCODE_NUMPAD_DIVIDE -> 0x54
            KEYCODE_NUMPAD_MULTIPLY -> 0x55
            KEYCODE_NUMPAD_SUBTRACT -> 0x56
            KEYCODE_NUMPAD_ADD -> 0x57
            KEYCODE_NUMPAD_ENTER -> 0x58
            in KEYCODE_NUMPAD_1..KEYCODE_NUMPAD_9 -> keyCode - KEYCODE_NUMPAD_1 + 0x59
            KEYCODE_NUMPAD_0 -> 0x62
            KEYCODE_NUMPAD_DOT -> 0x63
            KEYCODE_MENU -> 0x65
            KEYCODE_CTRL_LEFT -> 0xE0
            KEYCODE_SHIFT_LEFT -> 0xE1
            KEYCODE_ALT_LEFT -> 0xE2
            KEYCODE_META_LEFT -> 0xE3
            KEYCODE_CTRL_RIGHT -> 0xE4
            KEYCODE_SHIFT_RIGHT -> 0xE5
            KEYCODE_ALT_RIGHT -> 0xE6
            KEYCODE_META_RIGHT -> 0xE7
            else -> null
        }

    fun mods(metaState: Int): Int =
        sideMods(metaState, META_CTRL_ON, META_CTRL_LEFT_ON, META_CTRL_RIGHT_ON, 0, 4) or
            sideMods(metaState, META_SHIFT_ON, META_SHIFT_LEFT_ON, META_SHIFT_RIGHT_ON, 1, 5) or
            sideMods(metaState, META_ALT_ON, META_ALT_LEFT_ON, META_ALT_RIGHT_ON, 2, 6) or
            sideMods(metaState, META_META_ON, META_META_LEFT_ON, META_META_RIGHT_ON, 3, 7)

    fun isModifier(keyCode: Int): Boolean =
        keyCode == KEYCODE_CTRL_LEFT ||
            keyCode == KEYCODE_SHIFT_LEFT ||
            keyCode == KEYCODE_ALT_LEFT ||
            keyCode == KEYCODE_META_LEFT ||
            keyCode == KEYCODE_CTRL_RIGHT ||
            keyCode == KEYCODE_SHIFT_RIGHT ||
            keyCode == KEYCODE_ALT_RIGHT ||
            keyCode == KEYCODE_META_RIGHT

    private fun sideMods(
        metaState: Int,
        generic: Int,
        left: Int,
        right: Int,
        leftBit: Int,
        rightBit: Int,
    ): Int {
        val sideState = metaState and (left or right)
        if (sideState == 0) return if (metaState and generic != 0) 1 shl leftBit else 0
        return (if (sideState and left != 0) 1 shl leftBit else 0) or
            (if (sideState and right != 0) 1 shl rightBit else 0)
    }

    // KeyEvent.KEYCODE_0 / KEYCODE_1 ... KEYCODE_9.
    private const val KEYCODE_0 = 7
    private const val KEYCODE_1 = 8
    private const val KEYCODE_9 = 16

    // KeyEvent.KEYCODE_DPAD_UP / DOWN / LEFT / RIGHT.
    private const val KEYCODE_DPAD_UP = 19
    private const val KEYCODE_DPAD_DOWN = 20
    private const val KEYCODE_DPAD_LEFT = 21
    private const val KEYCODE_DPAD_RIGHT = 22

    // KeyEvent.KEYCODE_A ... KEYCODE_Z.
    private const val KEYCODE_A = 29
    private const val KEYCODE_Z = 54

    // KeyEvent punctuation and editing key codes.
    private const val KEYCODE_COMMA = 55
    private const val KEYCODE_PERIOD = 56
    private const val KEYCODE_ALT_LEFT = 57
    private const val KEYCODE_ALT_RIGHT = 58
    private const val KEYCODE_SHIFT_LEFT = 59
    private const val KEYCODE_SHIFT_RIGHT = 60
    private const val KEYCODE_TAB = 61
    private const val KEYCODE_SPACE = 62
    private const val KEYCODE_ENTER = 66
    private const val KEYCODE_DEL = 67
    private const val KEYCODE_GRAVE = 68
    private const val KEYCODE_MINUS = 69
    private const val KEYCODE_EQUALS = 70
    private const val KEYCODE_LEFT_BRACKET = 71
    private const val KEYCODE_RIGHT_BRACKET = 72
    private const val KEYCODE_BACKSLASH = 73
    private const val KEYCODE_SEMICOLON = 74
    private const val KEYCODE_APOSTROPHE = 75
    private const val KEYCODE_SLASH = 76
    private const val KEYCODE_MENU = 82
    private const val KEYCODE_PAGE_UP = 92
    private const val KEYCODE_PAGE_DOWN = 93
    private const val KEYCODE_ESCAPE = 111
    private const val KEYCODE_FORWARD_DEL = 112
    private const val KEYCODE_CTRL_LEFT = 113
    private const val KEYCODE_CTRL_RIGHT = 114
    private const val KEYCODE_CAPS_LOCK = 115
    private const val KEYCODE_SCROLL_LOCK = 116
    private const val KEYCODE_META_LEFT = 117
    private const val KEYCODE_META_RIGHT = 118
    private const val KEYCODE_SYSRQ = 120
    private const val KEYCODE_BREAK = 121
    private const val KEYCODE_MOVE_HOME = 122
    private const val KEYCODE_MOVE_END = 123
    private const val KEYCODE_INSERT = 124

    // KeyEvent.KEYCODE_F1 ... KEYCODE_F12.
    private const val KEYCODE_F1 = 131
    private const val KEYCODE_F12 = 142

    // KeyEvent numpad key codes.
    private const val KEYCODE_NUMPAD_0 = 144
    private const val KEYCODE_NUMPAD_1 = 145
    private const val KEYCODE_NUMPAD_9 = 153
    private const val KEYCODE_NUMPAD_DIVIDE = 154
    private const val KEYCODE_NUMPAD_MULTIPLY = 155
    private const val KEYCODE_NUMPAD_SUBTRACT = 156
    private const val KEYCODE_NUMPAD_ADD = 157
    private const val KEYCODE_NUMPAD_DOT = 158
    private const val KEYCODE_NUMPAD_ENTER = 160
    private const val KEYCODE_NUM_LOCK = 143

    // KeyEvent.META_SHIFT_ON / META_ALT_ON.
    private const val META_SHIFT_ON = 0x1
    private const val META_ALT_ON = 0x2

    // KeyEvent.META_ALT_LEFT_ON / META_ALT_RIGHT_ON.
    private const val META_ALT_LEFT_ON = 0x10
    private const val META_ALT_RIGHT_ON = 0x20

    // KeyEvent.META_SHIFT_LEFT_ON / META_SHIFT_RIGHT_ON.
    private const val META_SHIFT_LEFT_ON = 0x40
    private const val META_SHIFT_RIGHT_ON = 0x80

    // KeyEvent.META_CTRL_ON / META_CTRL_LEFT_ON / META_CTRL_RIGHT_ON.
    private const val META_CTRL_ON = 0x1000
    private const val META_CTRL_LEFT_ON = 0x2000
    private const val META_CTRL_RIGHT_ON = 0x4000

    // KeyEvent.META_META_ON / META_META_LEFT_ON / META_META_RIGHT_ON.
    private const val META_META_ON = 0x10000
    private const val META_META_LEFT_ON = 0x20000
    private const val META_META_RIGHT_ON = 0x40000
}
