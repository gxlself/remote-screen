package ai.gxlself.remotescreen.rsnative.recording

import android.app.Application
import ai.gxlself.remotescreen.rsnative.common.RsLog

/** Registers the native recording key wrapping callback during application startup. */
object RecordingKeyBootstrap {
  @Suppress("UNUSED_PARAMETER")
  fun start(app: Application) {
    val registered = RecordingKeyNative.register()
    RsLog.marker("BOOT", mapOf("recordingKey" to if (registered) "registered" else "failed"))
  }
}
