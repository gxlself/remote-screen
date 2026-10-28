package ai.gxlself.remotescreen.logic.keyboard

object HardwareKeyPlanner {
    /** Returns null when the key is unknown and should be handled by Android. */
    fun plan(
        keyCode: Int,
        down: Boolean,
        metaState: Int,
        os: KeyboardOs,
        stickyMods: Int,
    ): List<KeyAction>? {
        val usage = AndroidKeyMap.usage(keyCode) ?: return null
        val androidMods = AndroidKeyMap.mods(metaState)
        val mods =
            if (AndroidKeyMap.isModifier(keyCode)) {
                Modifiers.after(usage, down, androidMods)
            } else {
                androidMods or if (down) stickyMods else 0
            }
        val (mappedUsage, mappedMods) = OsMap.remap(usage, mods, os)
        val actions = mutableListOf<KeyAction>(KeyAction.Key(mappedUsage, down, mappedMods))
        if (!AndroidKeyMap.isModifier(keyCode) && down && stickyMods != 0) {
            actions += KeyAction.StickyConsumed(stickyMods)
        }
        return actions
    }
}
