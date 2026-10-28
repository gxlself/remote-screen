package ai.gxlself.remotescreen.rsnative.jni

interface SessionSink {
  fun onStreamCrop(
    epoch: Int,
    srcX: Float,
    srcY: Float,
    srcW: Float,
    srcH: Float,
    displayW: Int,
    displayH: Int,
  )

  // Called on a Rust worker. Arrays are callback-local copies and may be kept.
  // Return promptly: never block here or synchronously wait for the main thread.
  // Unsigned C fields preserve their bits; use toUInt()/toULong() as needed.
  fun onFrame(
    frameId: Int, keyframe: Boolean, displayId: Int, epoch: Int, codec: Int,
    clockSynced: Boolean, captureTsUs: Long, captureLocalUs: Long, receivedUs: Long,
    vps: ByteArray?, sps: ByteArray?, pps: ByteArray?, avcc: ByteArray,
  )

  // message contains UTF-8 bytes, including characters outside Modified UTF-8.
  fun onEvent(
    kind: Int, width: Int, height: Int, fps: Int, displayId: Int, epoch: Int, codec: Int,
    cursorVisible: Boolean, cursorX: Float, cursorY: Float, rttUs: Long, offsetUs: Long,
    code: Int, shapeId: Long, hotX: Int, hotY: Int, scale: Float,
    data: ByteArray?, message: ByteArray?,
  )

  companion object {
    const val EV_STREAM_CONFIG = 2
    const val EV_CLOCK_SYNC = 3
    const val EV_CLOSED = 5
    const val EV_CURSOR_POS = 6
    const val EV_CURSOR_SHAPE = 7
    const val EV_CURSOR_USE = 8
    const val EV_CURSOR_HIDDEN = 9
    const val CODEC_HEVC = 1
    const val CODEC_H264 = 2
    const val KEYFRAME_DECODER_RESET = 2
    const val KEYFRAME_BACKLOG = 4
  }
}
