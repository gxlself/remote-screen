package ai.gxlself.remotescreen.logic.terminal

data class TermRgb(val r: Int, val g: Int, val b: Int) {
    fun argb(): Int =
        0xff000000L.toInt() or
            ((r and 0xff) shl 16) or
            ((g and 0xff) shl 8) or
            (b and 0xff)
}

data class TermTheme(
    val foreground: TermRgb,
    val background: TermRgb,
    val cursor: TermRgb,
    val ansi: List<TermRgb>,
) {
    companion object {
        val NIGHT =
            TermTheme(
                foreground = rgb("#D5DCE0"),
                background = rgb("#07090B"),
                cursor = rgb("#ECF0F2"),
                ansi =
                    listOf(
                        rgb("#1C2328"),
                        rgb("#FF8873"),
                        rgb("#5DD39A"),
                        rgb("#F4B24E"),
                        rgb("#7DA8FF"),
                        rgb("#D69CFF"),
                        rgb("#63D7D7"),
                        rgb("#D5DCE0"),
                        rgb("#86929A"),
                        rgb("#FF8873"),
                        rgb("#5DD39A"),
                        rgb("#F4B24E"),
                        rgb("#7DA8FF"),
                        rgb("#E1B5FF"),
                        rgb("#79E8E8"),
                        rgb("#F4F7F8"),
                    ),
            )

        fun rgb(hex: String): TermRgb {
            val value = hex.trim().dropWhile { it == '#' }
            if (value.length != 6) return TermRgb(0, 0, 0)
            val number = value.toIntOrNull(16) ?: return TermRgb(0, 0, 0)
            return TermRgb(
                r = (number shr 16) and 0xff,
                g = (number shr 8) and 0xff,
                b = number and 0xff,
            )
        }

        fun named(name: String): TermTheme = NIGHT
    }
}
