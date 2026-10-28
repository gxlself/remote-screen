package ai.gxlself.remotescreen.rsnative.jni

import java.nio.ByteBuffer

/** Adapter-session sink: events as for SessionSink, frames as "new content" notifications. */
interface RawSink : SessionSink {
  // Called on a Rust worker; seq is unsigned and increases from 1. Only post work here:
  // never block, wait for the main thread or call RsRawNative.copy.
  fun onRawReady(seq: Long, width: Int, height: Int)
}

object RsRawNative {
  init {
    RsNative.ensureLoaded()
  }

  fun attach(id: Long, sink: RawSink): Boolean = nativeAttach(id, sink)

  // Call from a lifecycle/Java thread, never from a RawSink callback.
  fun detach(id: Long) = nativeDetach(id)

  /**
   * Copies RGBA8 pixels changed since [sinceSeq] into [dst], which must be a direct
   * ByteBuffer. [rect] needs at least 4 elements and receives [x, y, w, h].
   * Returns the copied seq, or 0 when nothing was copied.
   */
  fun copy(
    id: Long,
    sinceSeq: Long,
    dst: ByteBuffer,
    stride: Int,
    width: Int,
    height: Int,
    rect: IntArray,
  ): Long = nativeCopy(id, sinceSeq, dst, stride, width, height, rect)

  @JvmStatic private external fun nativeAttach(id: Long, sink: RawSink): Boolean
  @JvmStatic private external fun nativeDetach(id: Long)
  @JvmStatic private external fun nativeCopy(
    id: Long,
    sinceSeq: Long,
    dst: ByteBuffer,
    stride: Int,
    width: Int,
    height: Int,
    rect: IntArray,
  ): Long
}
