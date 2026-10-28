import Foundation
import LocalAuthentication

@objc(LocalAuthService)
final class LocalAuthService: NSObject {
    @objc(shared)
    static let shared = LocalAuthService()

    private var completions: [(String) -> Void] = []
    private var isEvaluating = false

    private override init() {}

    /// Starts one device-owner authentication request and shares its result
    /// with callers that arrive while that request is still in flight.
    @objc(authenticateWithReason:fallbackTitle:completion:)
    func authenticate(
        reason: String,
        fallbackTitle: String,
        completion: @escaping (String) -> Void
    ) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.completions.append(completion)
            guard !self.isEvaluating else { return }
            self.isEvaluating = true
            self.evaluate(reason: reason, fallbackTitle: fallbackTitle)
        }
    }

    @objc(getBiometryType)
    func getBiometryType() -> String {
        let context = LAContext()
        _ = context.canEvaluatePolicy(
            .deviceOwnerAuthenticationWithBiometrics,
            error: nil
        )
        return LocalAuthBiometry.name(rawType: context.biometryType.rawValue)
    }

    @objc(canAuthenticate)
    func canAuthenticate() -> NSDictionary {
        let context = LAContext()
        let capabilities = capabilities(for: context)
        return [
            "biometrics": capabilities.biometrics,
            "passcode": capabilities.passcode,
        ] as NSDictionary
    }

    private func evaluate(reason: String, fallbackTitle: String) {
        let context = LAContext()
        context.localizedFallbackTitle = fallbackTitle
        let capabilities = capabilities(for: context)
        let biometry = LocalAuthBiometry.name(rawType: context.biometryType.rawValue)

        // Avoid presenting a policy sheet when neither biometrics nor the
        // device passcode can be evaluated on this device.
        guard capabilities.biometrics || capabilities.passcode else {
            let code = capabilities.errorCode ?? -5
            finish(result: "unavailable", code: code, biometry: biometry)
            return
        }

        context.evaluatePolicy(
            .deviceOwnerAuthentication,
            localizedReason: reason
        ) { [weak self] success, error in
            DispatchQueue.main.async {
                guard let self else { return }
                let code = success ? 0 : ((error as NSError?)?.code ?? -999)
                let result: String
                if success {
                    result = "ok"
                } else {
                    result = LocalAuthErrorMap.result(
                        code: code,
                        canBiometrics: capabilities.biometrics,
                        canPasscode: capabilities.passcode
                    )
                }
                self.finish(result: result, code: code, biometry: biometry)
            }
        }
    }

    private func capabilities(for context: LAContext) -> Capabilities {
        var biometryError: NSError?
        let biometrics = context.canEvaluatePolicy(
            .deviceOwnerAuthenticationWithBiometrics,
            error: &biometryError
        )
        var passcodeError: NSError?
        let passcode = context.canEvaluatePolicy(
            .deviceOwnerAuthentication,
            error: &passcodeError
        )
        return Capabilities(
            biometrics: biometrics,
            passcode: passcode,
            errorCode: passcodeError?.code ?? biometryError?.code
        )
    }

    private func finish(result: String, code: Int, biometry: String) {
        assert(Thread.isMainThread)
        NSLog(
            "RS-LOCALAUTH {\"result\":\"%@\",\"code\":%ld,\"biometry\":\"%@\"}",
            result,
            code,
            biometry
        )
        let pending = completions
        completions.removeAll(keepingCapacity: false)
        isEvaluating = false
        pending.forEach { $0(result) }
    }

    private struct Capabilities {
        let biometrics: Bool
        let passcode: Bool
        let errorCode: Int?
    }
}
