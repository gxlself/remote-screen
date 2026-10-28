package ai.gxlself.remotescreen.rsnative.session.zoom

import ai.gxlself.remotescreen.rsnative.session.gestures.SessionGestureController

/** 安装会话触摸缩放处理。 */
object TouchZoomInstaller {
  fun install(c: SessionGestureController) {
    val zoom = ZoomController(c.sessionView)
    c.zoomHandler = zoom
    c.touchMode = TouchModeGestures(c.sessionView, c)
  }
}
