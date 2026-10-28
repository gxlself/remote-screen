package ai.gxlself.remotescreen.rsnative.jni

object RsSessionNative {
  init {
    RsNative.ensureLoaded()
  }

  fun attach(sessionId: Long, sink: SessionSink): Boolean = nativeAttach(sessionId, sink)

  // Call from a lifecycle/Java thread, never from a SessionSink callback.
  fun detach(sessionId: Long) = nativeDetach(sessionId)

  fun pointer(id: Long, x: Float, y: Float) = nativePointer(id, x, y)

  fun setViewport(
    id: Long,
    x: Float,
    y: Float,
    w: Float,
    h: Float,
    viewWPx: Int,
    viewHPx: Int,
    flags: Int,
  ) = nativeSetViewport(id, x, y, w, h, viewWPx, viewHPx, flags)

  fun button(id: Long, button: Int, down: Boolean, x: Float, y: Float, clicks: Int) =
    nativeButton(id, button, down, x, y, clicks.coerceIn(0, 255))

  fun scroll(id: Long, dx: Float, dy: Float, phase: Int, momentum: Int) =
    nativeScroll(id, dx, dy, phase.coerceIn(0, 255), momentum.coerceIn(0, 255))

  fun touch(id: Long, touchId: Int, phase: Int, x: Float, y: Float, pressure: Float) =
    nativeTouch(id, touchId.coerceIn(0, 255), phase.coerceIn(0, 255), x, y, pressure)

  fun releaseAll(id: Long) = nativeReleaseAll(id)

  fun requestKeyframe(id: Long, reason: Int) = nativeRequestKeyframe(id, reason.coerceIn(0, 255))

  fun reportDecoded(id: Long, decodeUs: Long, latencyUs: Long, queue: Int) =
    nativeReportDecoded(id, decodeUs, latencyUs, queue)

  @JvmStatic private external fun nativeAttach(sessionId: Long, sink: SessionSink): Boolean
  @JvmStatic private external fun nativeDetach(sessionId: Long)
  @JvmStatic private external fun nativePointer(id: Long, x: Float, y: Float)
  @JvmStatic private external fun nativeSetViewport(
    id: Long,
    x: Float,
    y: Float,
    w: Float,
    h: Float,
    viewWPx: Int,
    viewHPx: Int,
    flags: Int,
  )
  @JvmStatic private external fun nativeButton(
    id: Long, button: Int, down: Boolean, x: Float, y: Float, clicks: Int,
  )
  @JvmStatic private external fun nativeScroll(id: Long, dx: Float, dy: Float, phase: Int, momentum: Int)
  @JvmStatic private external fun nativeTouch(
    id: Long, touchId: Int, phase: Int, x: Float, y: Float, pressure: Float,
  )
  @JvmStatic private external fun nativeReleaseAll(id: Long)
  @JvmStatic private external fun nativeRequestKeyframe(id: Long, reason: Int)
  @JvmStatic private external fun nativeReportDecoded(id: Long, decodeUs: Long, latencyUs: Long, queue: Int)

  const val VIEWPORT_ACTIVE = 1
  const val VIEWPORT_CROP_OK = 2
}
