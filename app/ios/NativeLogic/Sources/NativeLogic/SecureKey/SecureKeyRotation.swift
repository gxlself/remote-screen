import Foundation

enum SecureKeyRotationAction: Equatable {
    case useExisting
    case create
    case deleteAndCreate
}

enum SecureKeyRotation {
    static func decide(installedMarker: Bool, keyExists: Bool) -> SecureKeyRotationAction {
        switch (installedMarker, keyExists) {
        case (true, true): return .useExisting
        case (true, false), (false, false): return .create
        case (false, true): return .deleteAndCreate
        }
    }
}
