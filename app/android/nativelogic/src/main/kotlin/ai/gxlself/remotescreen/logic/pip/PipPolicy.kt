package ai.gxlself.remotescreen.logic.pip

/** Platform policy for entering picture-in-picture when the user leaves the app. */
object PipPolicy {
  fun enterOnLeaveHint(sdk: Int, eligible: Boolean, supported: Boolean): Boolean =
    eligible && supported && sdk < 31
}
