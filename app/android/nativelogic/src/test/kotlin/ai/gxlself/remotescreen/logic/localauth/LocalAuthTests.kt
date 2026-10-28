package ai.gxlself.remotescreen.logic.localauth

import org.junit.Assert.assertEquals
import org.junit.Test

/** Android counterparts of the six iOS LocalAuthTests cases. */
class LocalAuthTests {
    @Test
    fun cancellationErrors() {
        for (code in listOf(10, 5, 13)) {
            assertEquals("cancelled", LocalAuthErrorMap.result(code, true, true))
        }
    }

    @Test
    fun authenticationFailures() {
        for (code in listOf(7, 9, 3, 2, 4, 8, 15)) {
            assertEquals("failed", LocalAuthErrorMap.result(code, true, true))
        }
    }

    @Test
    fun noDeviceCredentialIsUnavailable() {
        assertEquals("unavailable", LocalAuthErrorMap.result(14, false, false))
    }

    @Test
    fun biometryAvailabilityDependsOnPasscode() {
        for (code in listOf(11, 12, 1)) {
            assertEquals("unavailable", LocalAuthErrorMap.result(code, false, false))
            assertEquals("failed", LocalAuthErrorMap.result(code, false, true))
        }
    }

    @Test
    fun unknownErrorIsFailure() {
        for (code in listOf(-999, 0, 6, 16, Int.MIN_VALUE, Int.MAX_VALUE)) {
            for (biometrics in listOf(false, true)) {
                for (passcode in listOf(false, true)) {
                    assertEquals("failed", LocalAuthErrorMap.result(code, biometrics, passcode))
                }
            }
        }
    }

    @Test
    fun biometryNames() {
        for (features in 0..7) {
            val fingerprint = features and 1 != 0
            val face = features and 2 != 0
            val iris = features and 4 != 0
            assertEquals("none", LocalAuthBiometry.name(false, fingerprint, face, iris))
            val expected = when (features) {
                1 -> "fingerprint"
                2 -> "face"
                4 -> "iris"
                else -> "biometric"
            }
            assertEquals(expected, LocalAuthBiometry.name(true, fingerprint, face, iris))
        }
    }
}
