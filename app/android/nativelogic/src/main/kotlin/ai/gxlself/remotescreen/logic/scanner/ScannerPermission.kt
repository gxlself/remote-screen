package ai.gxlself.remotescreen.logic.scanner

enum class ScannerPermissionStatus {
  AUTHORIZED,
  NOT_DETERMINED,
  DENIED,
  RESTRICTED,
}

enum class ScannerPermissionDecision {
  START,
  REQUEST,
  DENIED,
  RESTRICTED,
  UNAVAILABLE,
}

object ScannerPermission {
  fun decide(
    status: ScannerPermissionStatus,
    hasDevice: Boolean,
  ): ScannerPermissionDecision {
    if (!hasDevice) return ScannerPermissionDecision.UNAVAILABLE
    return when (status) {
      ScannerPermissionStatus.AUTHORIZED -> ScannerPermissionDecision.START
      ScannerPermissionStatus.NOT_DETERMINED -> ScannerPermissionDecision.REQUEST
      ScannerPermissionStatus.DENIED -> ScannerPermissionDecision.DENIED
      ScannerPermissionStatus.RESTRICTED -> ScannerPermissionDecision.RESTRICTED
    }
  }

  fun fromAndroid(granted: Boolean, policyDisabled: Boolean): ScannerPermissionStatus = when {
    policyDisabled -> ScannerPermissionStatus.RESTRICTED
    granted -> ScannerPermissionStatus.AUTHORIZED
    else -> ScannerPermissionStatus.NOT_DETERMINED
  }
}
