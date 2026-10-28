package ai.gxlself.remotescreen.logic.securekey

import kotlin.math.ceil
import kotlin.math.round

object SignStats {
  data class Summary(val p50: Double, val p95: Double, val max: Double)

  fun summary(samplesMs: List<Double>): Summary {
    if (samplesMs.isEmpty()) return Summary(0.0, 0.0, 0.0)
    val sorted = samplesMs.sorted()

    fun nearestRank(percentile: Double): Double {
      val rank = maxOf(1, ceil(percentile * sorted.size).toInt())
      return sorted[rank - 1]
    }

    return Summary(nearestRank(0.50), nearestRank(0.95), sorted.last())
  }

  fun round2(value: Double): Double = round(value * 100.0) / 100.0
}
