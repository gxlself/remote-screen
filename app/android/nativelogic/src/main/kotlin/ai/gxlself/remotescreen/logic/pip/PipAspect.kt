package ai.gxlself.remotescreen.logic.pip

/** Keeps a remote display aspect ratio within Android's picture-in-picture limits. */
object PipAspect {
  private const val MAX_RATIO_NUMERATOR = 239L
  private const val MAX_RATIO_DENOMINATOR = 100L

  fun clamp(w: Int, h: Int): Pair<Int, Int> {
    if (w <= 0 || h <= 0) {
      return 16 to 9
    }

    val width = w.toLong()
    val height = h.toLong()
    return when {
      width * MAX_RATIO_DENOMINATOR > height * MAX_RATIO_NUMERATOR ->
        (height * MAX_RATIO_NUMERATOR / MAX_RATIO_DENOMINATOR).toInt().coerceAtLeast(1) to h
      height * MAX_RATIO_DENOMINATOR > width * MAX_RATIO_NUMERATOR ->
        w to (width * MAX_RATIO_NUMERATOR / MAX_RATIO_DENOMINATOR).toInt().coerceAtLeast(1)
      else -> w to h
    }
  }
}
