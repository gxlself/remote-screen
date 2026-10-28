package ai.gxlself.remotescreen.logic.keyboard

object Modifiers {
    /** Returns the wire mask after this transition, including the modifier key itself. */
    fun after(usage: Int, down: Boolean, current: Int): Int {
        if (usage !in 0xE0..0xE7) return current
        val bit = 1 shl (usage - 0xE0)
        return if (down) current or bit else current and bit.inv()
    }
}
