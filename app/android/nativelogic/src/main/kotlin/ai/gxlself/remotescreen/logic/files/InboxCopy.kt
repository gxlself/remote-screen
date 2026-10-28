package ai.gxlself.remotescreen.logic.files

import java.io.File
import java.io.IOException
import java.io.InputStream

/** Reserves a unique inbox entry and removes it if opening or streaming the source fails. */
object InboxCopy {
  fun copy(directory: File, name: String?, openSource: () -> InputStream): File {
    val destination = reserveDestination(directory, name)
    try {
      openSource().use { input ->
        destination.outputStream().use { output -> input.copyTo(output, 64 * 1024) }
      }
      return destination
    } catch (error: Throwable) {
      destination.delete()
      throw error
    }
  }

  private fun reserveDestination(directory: File, name: String?): File {
    while (true) {
      val taken = directory.list()?.toSet() ?: throw IOException("Unable to list inbox directory")
      val destination = File(directory, FileNameRules.unique(name, taken))
      if (destination.createNewFile()) return destination
    }
  }
}
