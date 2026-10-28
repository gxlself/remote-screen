package ai.gxlself.remotescreen.logic.keyboard

enum class InputMode {
    TEXT,
    KEYS,
}

enum class KeyboardOs {
    MACOS,
    WINDOWS,
}

sealed interface KeyAction {
    data class Text(val text: String) : KeyAction

    data class Key(val usage: Int, val down: Boolean, val mods: Int) : KeyAction

    data class StickyConsumed(val mods: Int) : KeyAction
}

fun interface GraphemeSplitter {
    fun clusters(text: String): List<String>
}
