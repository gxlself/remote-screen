package ai.gxlself.remotescreen.logic.system

/** Converts Android notification state into the shared JavaScript result values. */
object NotificationStatus {
  fun status(
    sdk: Int,
    permissionGranted: Boolean,
    notificationsEnabled: Boolean,
    requestedBefore: Boolean,
  ): String {
    if (!notificationsEnabled) {
      return "denied"
    }
    if (sdk < 33 || permissionGranted) {
      return "granted"
    }
    return if (requestedBefore) "denied" else "notDetermined"
  }
}
