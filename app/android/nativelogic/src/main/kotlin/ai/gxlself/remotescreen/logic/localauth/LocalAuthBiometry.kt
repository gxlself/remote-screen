package ai.gxlself.remotescreen.logic.localauth

object LocalAuthBiometry {
    fun name(enrolled: Boolean, fingerprint: Boolean, face: Boolean, iris: Boolean): String = when {
        !enrolled -> "none"
        fingerprint && !face && !iris -> "fingerprint"
        face && !fingerprint && !iris -> "face"
        iris && !fingerprint && !face -> "iris"
        // Also covers devices reporting enrolled biometrics without a known feature flag.
        else -> "biometric"
    }
}
