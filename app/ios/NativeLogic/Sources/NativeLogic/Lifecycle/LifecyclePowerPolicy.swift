import Foundation

public enum BatteryStateKind: Equatable {
    case unknown
    case unplugged
    case charging
    case full

    public var isCharging: Bool {
        self == .charging || self == .full
    }
}

public struct LifecyclePowerState: Equatable {
    public let lowPower: Bool
    public let lowPowerMode: Bool
    public let batteryLevel: Double
    public let charging: Bool

    public init(
        lowPower: Bool,
        lowPowerMode: Bool,
        batteryLevel: Double,
        charging: Bool
    ) {
        self.lowPower = lowPower
        self.lowPowerMode = lowPowerMode
        self.batteryLevel = batteryLevel
        self.charging = charging
    }
}

public enum LifecyclePowerPolicy {
    public static let lowBatteryThreshold = 0.20
    public static let changeThreshold = 0.05

    public static func state(
        lowPowerMode: Bool,
        level: Double,
        batteryState: BatteryStateKind
    ) -> LifecyclePowerState {
        let charging = batteryState.isCharging
        let lowBattery = level >= 0 && level < lowBatteryThreshold && !charging
        return LifecyclePowerState(
            lowPower: lowPowerMode || lowBattery,
            lowPowerMode: lowPowerMode,
            batteryLevel: level,
            charging: charging
        )
    }

    public static func changed(
        _ old: LifecyclePowerState,
        _ new: LifecyclePowerState
    ) -> Bool {
        if old.lowPower != new.lowPower || old.lowPowerMode != new.lowPowerMode {
            return true
        }
        if old.charging != new.charging {
            return true
        }
        guard old.batteryLevel >= 0, new.batteryLevel >= 0 else {
            return old.batteryLevel != new.batteryLevel
        }
        return abs(old.batteryLevel - new.batteryLevel) >= changeThreshold
    }
}
