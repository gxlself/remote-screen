import Foundation

enum SystemHapticKind: String, Equatable {
    case light
    case success
    case warning
    case error

    static func parse(_ raw: String) -> SystemHapticKind? {
        SystemHapticKind(rawValue: raw)
    }
}

typealias HapticKind = SystemHapticKind
