package ai.gxlself.remotescreen.logic.system

import kotlin.math.roundToInt

/** Returns a usable integer maximum refresh rate from Android display modes. */
object MaxFps {
  fun maxFps(refreshRates: List<Float>): Int =
    refreshRates.filter { it.isFinite() }.maxOrNull()?.roundToInt() ?: 60
}
