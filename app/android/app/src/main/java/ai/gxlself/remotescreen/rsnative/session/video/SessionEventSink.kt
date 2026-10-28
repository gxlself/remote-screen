package ai.gxlself.remotescreen.rsnative.session.video

import ai.gxlself.remotescreen.logic.session.VideoStatsSnapshot

/** All callbacks run on the main thread and are discarded after stop(). */
interface SessionEventSink {
  fun onVideoSize(width: Int, height: Int) {}
  fun onFirstFrame(epoch: Int, width: Int, height: Int) {}
  fun onVideoStats(stats: VideoStatsSnapshot) {}
  fun onCursorEvent(
    kind: Int, visible: Boolean, x: Float, y: Float, shapeId: Long,
    hotX: Int, hotY: Int, scale: Float, png: ByteArray?,
  ) {}
  fun onClosed(reason: String) {}
}
