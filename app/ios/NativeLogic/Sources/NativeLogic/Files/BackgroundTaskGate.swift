import Foundation

public struct BackgroundTaskGate {
    public enum Action: Equatable {
        case begin
        case end
        case none
    }

    private var active = false

    public init() {}

    public mutating func set(active requested: Bool) -> Action {
        if requested {
            guard !active else { return .none }
            active = true
            return .begin
        }

        guard active else { return .none }
        active = false
        return .end
    }

    public mutating func expired() -> Action {
        guard active else { return .none }
        active = false
        return .end
    }
}
