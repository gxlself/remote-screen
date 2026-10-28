package ai.gxlself.remotescreen.logic.localauth

/** Numeric BiometricPrompt errors, kept independent of the Android runtime. */
object LocalAuthErrorMap {
    private const val HW_UNAVAILABLE = 1 // BiometricPrompt.ERROR_HW_UNAVAILABLE
    private const val UNABLE_TO_PROCESS = 2 // BiometricPrompt.ERROR_UNABLE_TO_PROCESS
    private const val TIMEOUT = 3 // BiometricPrompt.ERROR_TIMEOUT
    private const val NO_SPACE = 4 // BiometricPrompt.ERROR_NO_SPACE
    private const val CANCELED = 5 // BiometricPrompt.ERROR_CANCELED
    private const val LOCKOUT = 7 // BiometricPrompt.ERROR_LOCKOUT
    private const val VENDOR = 8 // BiometricPrompt.ERROR_VENDOR
    private const val LOCKOUT_PERMANENT = 9 // BiometricPrompt.ERROR_LOCKOUT_PERMANENT
    private const val USER_CANCELED = 10 // BiometricPrompt.ERROR_USER_CANCELED
    private const val NO_BIOMETRICS = 11 // BiometricPrompt.ERROR_NO_BIOMETRICS
    private const val HW_NOT_PRESENT = 12 // BiometricPrompt.ERROR_HW_NOT_PRESENT
    private const val NEGATIVE_BUTTON = 13 // BiometricPrompt.ERROR_NEGATIVE_BUTTON
    private const val NO_DEVICE_CREDENTIAL = 14 // BiometricPrompt.ERROR_NO_DEVICE_CREDENTIAL
    private const val SECURITY_UPDATE_REQUIRED = 15 // BiometricPrompt.ERROR_SECURITY_UPDATE_REQUIRED

    // canBiometrics preserves the shared policy interface; only passcode
    // availability changes how an unavailable biometric sensor is reported.
    fun result(
        code: Int,
        @Suppress("UNUSED_PARAMETER") canBiometrics: Boolean,
        canPasscode: Boolean,
    ): String = when (code) {
        USER_CANCELED, CANCELED, NEGATIVE_BUTTON -> "cancelled"
        NO_DEVICE_CREDENTIAL -> "unavailable"
        NO_BIOMETRICS, HW_NOT_PRESENT, HW_UNAVAILABLE -> if (canPasscode) "failed" else "unavailable"
        LOCKOUT, LOCKOUT_PERMANENT, TIMEOUT, UNABLE_TO_PROCESS, NO_SPACE, VENDOR,
        SECURITY_UPDATE_REQUIRED -> "failed"
        else -> "failed"
    }
}
