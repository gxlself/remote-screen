import Foundation

enum ScannerPermissionStatus: Equatable {
    case authorized
    case notDetermined
    case denied
    case restricted
}

enum ScannerPermissionDecision: Equatable {
    case start
    case request
    case denied
    case restricted
    case unavailable
}

enum ScannerPermission {
    static func decide(
        _ status: ScannerPermissionStatus,
        hasDevice: Bool
    ) -> ScannerPermissionDecision {
        guard hasDevice else { return .unavailable }
        switch status {
        case .authorized:
            return .start
        case .notDetermined:
            return .request
        case .denied:
            return .denied
        case .restricted:
            return .restricted
        }
    }

    static func decide(
        status: ScannerPermissionStatus,
        hasDevice: Bool
    ) -> ScannerPermissionDecision {
        decide(status, hasDevice: hasDevice)
    }
}
