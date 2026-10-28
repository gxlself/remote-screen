package ai.gxlself.remotescreen.rsnative.jni

object RsTermNative {
  init {
    RsNative.ensureLoaded()
  }

  fun attach(sshId: Long, sink: TermSink): Boolean = nativeAttach(sshId, sink)

  // Detach is safe from a TermSink callback; the registry defers context cleanup.
  fun detach(sshId: Long) = nativeDetach(sshId)

  fun input(sshId: Long, data: ByteArray) {
    if (data.isNotEmpty()) {
      nativeInput(sshId, data)
    }
  }

  fun resize(sshId: Long, cols: Int, rows: Int) {
    val clampedCols = cols.coerceIn(0, 0xFFFF)
    val clampedRows = rows.coerceIn(0, 0xFFFF)
    if (clampedCols != 0 && clampedRows != 0) {
      nativeResize(sshId, clampedCols, clampedRows)
    }
  }

  @JvmStatic private external fun nativeAttach(sshId: Long, sink: TermSink): Boolean
  @JvmStatic private external fun nativeDetach(sshId: Long)
  @JvmStatic private external fun nativeInput(sshId: Long, data: ByteArray)
  @JvmStatic private external fun nativeResize(sshId: Long, cols: Int, rows: Int)
}
