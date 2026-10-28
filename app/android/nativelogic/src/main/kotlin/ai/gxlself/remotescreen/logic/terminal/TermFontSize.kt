package ai.gxlself.remotescreen.logic.terminal

object TermFontSize {
    fun clamp(v: Double): Double {
        if (!v.isFinite()) return DEFAULT
        return v.coerceIn(MIN, MAX)
    }

    private const val MIN = 9.0
    private const val MAX = 24.0
    private const val DEFAULT = 13.0
}
