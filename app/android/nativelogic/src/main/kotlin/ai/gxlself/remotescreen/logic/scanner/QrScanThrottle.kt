package ai.gxlself.remotescreen.logic.scanner

/** Suppresses the same QR value for a short, caller-provided time window. */
class QrScanThrottle(val windowMs: Long = 2_000) {
  private val acceptedAtMs = mutableMapOf<String, Long>()

  fun accept(text: String, nowMs: Long): Boolean {
    acceptedAtMs.entries.removeIf { (_, accepted) ->
      nowMs < accepted || nowMs - accepted >= windowMs
    }
    if (acceptedAtMs.containsKey(text)) return false
    acceptedAtMs[text] = nowMs
    return true
  }
}
