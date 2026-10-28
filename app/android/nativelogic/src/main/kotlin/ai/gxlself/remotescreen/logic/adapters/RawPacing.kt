package ai.gxlself.remotescreen.logic.adapters

/**
 * Coalesces raw-frame ready notifications into paced pulls, with the same semantics
 * as iOS AdapterRawPacing. Times are milliseconds; seqs are unsigned.
 * Not thread-safe: callers hold their own lock.
 */
class RawPacing(private val minIntervalMs: Long = 16) {
  private var latestSeq = 0L
  private var queued = false
  private var pulling = false
  private var lastBeginMs: Long? = null

  init {
    require(minIntervalMs >= 0) { "minIntervalMs must not be negative" }
  }

  /** Records the newest seq. Returns the delay of a newly scheduled pull, or null if one is queued or running. */
  fun ready(seq: Long, nowMs: Long): Long? {
    if (java.lang.Long.compareUnsigned(seq, latestSeq) > 0) latestSeq = seq
    if (queued || pulling) return null
    queued = true
    return nextDelay(nowMs)
  }

  /** Starts the scheduled pull and returns the newest seq known so far. */
  fun begin(nowMs: Long): Long {
    queued = false
    pulling = true
    lastBeginMs = nowMs
    return latestSeq
  }

  /** Completes a pull. A successful pull behind the newest seq schedules one more pull. */
  fun end(pulledSeq: Long, nowMs: Long): Long? {
    pulling = false
    if (pulledSeq == 0L || java.lang.Long.compareUnsigned(latestSeq, pulledSeq) <= 0) return null
    queued = true
    return nextDelay(nowMs)
  }

  fun reset() {
    latestSeq = 0
    queued = false
    pulling = false
    lastBeginMs = null
  }

  private fun nextDelay(nowMs: Long): Long {
    val last = lastBeginMs ?: return 0
    return (last + minIntervalMs - nowMs).coerceAtLeast(0)
  }
}
