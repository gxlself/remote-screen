package ai.gxlself.remotescreen.logic.terminal

object TermKeySequences {
    fun bytes(name: String, appCursor: Boolean, mods: Int): ByteArray {
        val modifier = xtermModifier(mods)
        return when (name) {
            "esc" -> TermModifierApply.apply(byteArrayOf(ESC), mods).bytes
            "tab" -> TermModifierApply.apply(byteArrayOf(TAB), mods).bytes
            "up" -> csi('A', appCursor, modifier)
            "down" -> csi('B', appCursor, modifier)
            "right" -> csi('C', appCursor, modifier)
            "left" -> csi('D', appCursor, modifier)
            "home" -> csi('H', appCursor, modifier)
            "end" -> csi('F', appCursor, modifier)
            "pageUp" -> tilde("5", mods)
            "pageDown" -> tilde("6", mods)
            else -> {
                if (name.codePointCount(0, name.length) != 1) {
                    byteArrayOf()
                } else {
                    TermModifierApply.apply(name.toByteArray(Charsets.UTF_8), mods).bytes
                }
            }
        }
    }

    internal fun xtermModifier(mods: Int): Int =
        1 + (if (mods and TermMods.ALT != 0) 2 else 0) +
            (if (mods and TermMods.CTRL != 0) 4 else 0)

    internal fun tilde(prefix: String, mods: Int): ByteArray =
        tildeWithModifier(prefix, xtermModifier(mods))

    private fun csi(final: Char, appCursor: Boolean, modifier: Int): ByteArray {
        val sequence =
            if (modifier == 1) {
                "\u001b${if (appCursor) 'O' else '['}$final"
            } else {
                "\u001b[1;$modifier$final"
            }
        return sequence.toByteArray(Charsets.UTF_8)
    }

    private fun tildeWithModifier(prefix: String, modifier: Int): ByteArray {
        val sequence =
            if (modifier == 1) "\u001b[$prefix~" else "\u001b[$prefix;$modifier~"
        return sequence.toByteArray(Charsets.UTF_8)
    }

    private const val ESC: Byte = 0x1b
    private const val TAB: Byte = 0x09
}
