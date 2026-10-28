package ai.gxlself.remotescreen.logic.system

/** The haptic kinds exposed by the cross-platform system module. */
enum class HapticKind {
  LIGHT,
  SUCCESS,
  WARNING,
  ERROR;

  companion object {
    /** Parses the wire spelling without accepting case variants or aliases. */
    fun parse(raw: String): HapticKind? = when (raw) {
      "light" -> LIGHT
      "success" -> SUCCESS
      "warning" -> WARNING
      "error" -> ERROR
      else -> null
    }
  }
}

/** Maps the platform-independent kinds to Android haptic constants. */
object HapticMap {
  fun constantFor(kind: HapticKind, sdk: Int): Int = when (kind) {
    HapticKind.LIGHT -> 3 // HapticFeedbackConstants.KEYBOARD_TAP
    HapticKind.SUCCESS -> if (sdk >= 30) 16 else 1 // CONFIRM / VIRTUAL_KEY
    HapticKind.WARNING -> 0 // HapticFeedbackConstants.LONG_PRESS
    HapticKind.ERROR -> if (sdk >= 30) 17 else 0 // REJECT / LONG_PRESS
  }
}
