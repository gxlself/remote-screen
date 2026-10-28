package ai.gxlself.remotescreen.logic.terminal

object TermGrid {
    fun size(
        widthPx: Int,
        heightPx: Int,
        cellWidthPx: Float,
        lineHeightPx: Int,
        paddingPx: Int = 0,
    ): Pair<Int, Int> {
        if (!cellWidthPx.isFinite() || cellWidthPx <= 0f || lineHeightPx <= 0) {
            return 1 to 1
        }

        val padding = paddingPx.coerceAtLeast(0).toLong()
        val usableWidth = (widthPx.toLong() - 2L * padding).coerceAtLeast(0L)
        val usableHeight = (heightPx.toLong() - 2L * padding).coerceAtLeast(0L)
        val cols =
            (usableWidth.toFloat() / cellWidthPx)
                .coerceIn(1f, MAX_COLS.toFloat())
                .toInt()
        val rows =
            (usableHeight / lineHeightPx.toLong())
                .coerceIn(1L, MAX_ROWS.toLong())
                .toInt()
        return cols to rows
    }

    private const val MAX_COLS = 500
    private const val MAX_ROWS = 300
}
