package ai.gxlself.remotescreen.rsnative.files

import android.content.Intent
import android.net.Uri
import android.os.Build

/** Intent helpers shared by the TurboModule and MainActivity launch path. */
internal object RsFilesIntents {
  fun isShareAction(intent: Intent): Boolean =
    intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE

  fun streamUris(intent: Intent): List<Uri> {
    if (!isShareAction(intent)) {
      return emptyList()
    }
    return try {
      val streams = if (intent.action == Intent.ACTION_SEND) {
        listOfNotNull(singleStream(intent))
      } else {
        multipleStreams(intent)
      }
      streams.filter { it.scheme.equals("content", true) || it.scheme.equals("file", true) }
    } catch (_: RuntimeException) {
      emptyList()
    }
  }

  private fun singleStream(intent: Intent): Uri? {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
    } else {
      @Suppress("DEPRECATION")
      intent.getParcelableExtra<android.os.Parcelable>(Intent.EXTRA_STREAM) as? Uri
    }
  }

  private fun multipleStreams(intent: Intent): List<Uri> {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)?.toList().orEmpty()
    } else {
      @Suppress("DEPRECATION")
      intent.getParcelableArrayListExtra<android.os.Parcelable>(Intent.EXTRA_STREAM)
        ?.filterIsInstance<Uri>().orEmpty()
    }
  }
}
