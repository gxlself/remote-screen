package ai.gxlself.remotescreen.logic.localauth

import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized

/** One test per Android error code, each checked against all capability combinations. */
@RunWith(Parameterized::class)
class LocalAuthErrorCodeTest(
    private val errorName: String,
    private val code: Int,
    private val withoutPasscode: String,
    private val withPasscode: String,
) {
    @Test
    fun resultForEveryCapabilityCombination() {
        for (biometrics in listOf(false, true)) {
            assertEquals(errorName, withoutPasscode, LocalAuthErrorMap.result(code, biometrics, false))
            assertEquals(errorName, withPasscode, LocalAuthErrorMap.result(code, biometrics, true))
        }
    }

    companion object {
        @JvmStatic
        @Parameterized.Parameters(name = "{0} ({1})")
        fun errors(): Collection<Array<Any>> = listOf(
            arrayOf("HW_UNAVAILABLE", 1, "unavailable", "failed"),
            arrayOf("UNABLE_TO_PROCESS", 2, "failed", "failed"),
            arrayOf("TIMEOUT", 3, "failed", "failed"),
            arrayOf("NO_SPACE", 4, "failed", "failed"),
            arrayOf("CANCELED", 5, "cancelled", "cancelled"),
            arrayOf("LOCKOUT", 7, "failed", "failed"),
            arrayOf("VENDOR", 8, "failed", "failed"),
            arrayOf("LOCKOUT_PERMANENT", 9, "failed", "failed"),
            arrayOf("USER_CANCELED", 10, "cancelled", "cancelled"),
            arrayOf("NO_BIOMETRICS", 11, "unavailable", "failed"),
            arrayOf("HW_NOT_PRESENT", 12, "unavailable", "failed"),
            arrayOf("NEGATIVE_BUTTON", 13, "cancelled", "cancelled"),
            arrayOf("NO_DEVICE_CREDENTIAL", 14, "unavailable", "unavailable"),
            arrayOf("SECURITY_UPDATE_REQUIRED", 15, "failed", "failed"),
        )
    }
}
