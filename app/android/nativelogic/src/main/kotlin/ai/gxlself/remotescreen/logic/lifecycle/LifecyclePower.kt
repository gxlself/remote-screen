package ai.gxlself.remotescreen.logic.lifecycle

/** The battery states used by the platform adapters. */
enum class BatteryKind {
  UNKNOWN,
  UNPLUGGED,
  CHARGING,
  FULL,
  ;

  val isCharging: Boolean
    get() = this == CHARGING || this == FULL
}

data class PowerState(
  val lowPower: Boolean,
  val lowPowerMode: Boolean,
  val batteryLevel: Double,
  val charging: Boolean,
)

/** Platform-neutral policy shared by Android and its unit tests. */
object PowerPolicy {
  const val LOW_BATTERY_THRESHOLD = 0.20
  const val CHANGE_THRESHOLD = 0.05
  const val lowBatteryThreshold = LOW_BATTERY_THRESHOLD
  const val changeThreshold = CHANGE_THRESHOLD

  fun state(lowPowerMode: Boolean, level: Double, kind: BatteryKind): PowerState {
    val charging = kind.isCharging
    val lowBattery = level >= 0.0 && level < LOW_BATTERY_THRESHOLD && !charging
    return PowerState(
      lowPower = lowPowerMode || lowBattery,
      lowPowerMode = lowPowerMode,
      batteryLevel = level,
      charging = charging,
    )
  }

  fun changed(old: PowerState, new: PowerState): Boolean {
    if (old.lowPower != new.lowPower || old.lowPowerMode != new.lowPowerMode) return true
    if (old.charging != new.charging) return true
    if (old.batteryLevel < 0.0 || new.batteryLevel < 0.0) {
      return old.batteryLevel != new.batteryLevel
    }
    return kotlin.math.abs(old.batteryLevel - new.batteryLevel) >= CHANGE_THRESHOLD
  }
}

/** Converts Android battery intent values without importing the Android SDK. */
object BatteryMapper {
  private const val STATUS_CHARGING = 2
  private const val STATUS_DISCHARGING = 3
  private const val STATUS_NOT_CHARGING = 4
  private const val STATUS_FULL = 5

  fun kind(status: Int, plugged: Int): BatteryKind = when (status) {
    STATUS_CHARGING -> BatteryKind.CHARGING
    STATUS_FULL -> BatteryKind.FULL
    STATUS_DISCHARGING, STATUS_NOT_CHARGING -> BatteryKind.UNPLUGGED
    else -> if (plugged != 0) BatteryKind.CHARGING else BatteryKind.UNKNOWN
  }

  fun level(level: Int, scale: Int): Double {
    if (level <= 0 || scale <= 0) return -1.0
    return (level.toDouble() / scale.toDouble()).coerceIn(0.0, 1.0)
  }
}
