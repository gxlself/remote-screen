package ai.gxlself.remotescreen.logic.securekey

enum class RotationAction {
  USE_EXISTING,
  CREATE,
  DELETE_AND_CREATE,
}

object Rotation {
  fun decide(installedMarker: Boolean, keyExists: Boolean): RotationAction =
    when {
      installedMarker && keyExists -> RotationAction.USE_EXISTING
      !installedMarker && keyExists -> RotationAction.DELETE_AND_CREATE
      else -> RotationAction.CREATE
    }
}
