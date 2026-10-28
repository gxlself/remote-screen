package ai.gxlself.remotescreen.logic.keyboard

object OsMap {
    fun remap(usage: Int, mods: Int, os: KeyboardOs): Pair<Int, Int> {
        if (os != KeyboardOs.WINDOWS) return usage to mods

        val mappedUsage =
            when (usage) {
                0xE3 -> 0xE0
                0xE7 -> 0xE4
                else -> usage
            }
        val mappedMods =
            (mods and 0x88.inv()) or
                (if (mods and 0x08 != 0) 0x01 else 0) or
                (if (mods and 0x80 != 0) 0x10 else 0)
        return mappedUsage to mappedMods
    }
}
