import Foundation

/// The result values shared by the iOS LocalAuthentication adapter and JS.
///
/// This type intentionally takes the numeric LAError code. NativeLogic is a
/// platform-independent Swift package, so the policy/error mapping can be
/// tested on macOS without importing LocalAuthentication.
enum LocalAuthErrorMap {
    static func result(
        code: Int,
        canBiometrics: Bool,
        canPasscode: Bool
    ) -> String {
        switch code {
        case -2, // userCancel
             -3, // userFallback
             -4, // systemCancel
             -9: // appCancel
            return "cancelled"
        case -5, // passcodeNotSet
             -1004: // notInteractive
            return "unavailable"
        case -6: // biometryNotAvailable
            return canPasscode ? "failed" : "unavailable"
        case -1, // authenticationFailed
             -7, // biometryNotEnrolled
             -8: // biometryLockout
            return "failed"
        default:
            return "failed"
        }
    }
}

enum LocalAuthBiometry {
    static func name(rawType: Int) -> String {
        switch rawType {
        case 1:
            return "touchID"
        case 2:
            return "faceID"
        case 4:
            return "opticID"
        default:
            return "none"
        }
    }
}
