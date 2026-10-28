package ai.gxlself.remotescreen.logic.launch

import java.io.File

object LaunchArgs {
  val VALUE_KEYS = listOf(
    "autoConnect",
    "devHarness",
    "harnessArg",
    "disconnectAfter",
    "selfTestMove",
  )

  fun properties(extras: Map<String, String?>): Map<String, String> {
    val result = linkedMapOf<String, String>()
    for (key in VALUE_KEYS) {
      val value = extras[key]
      if (value != null && value.isNotEmpty()) {
        result[key] = value
      }
    }
    return result
  }
}

object LaunchDataDir {
  fun dir(noBackupDir: File): File = File(File(noBackupDir, "RemoteScreen"), "core")
}
