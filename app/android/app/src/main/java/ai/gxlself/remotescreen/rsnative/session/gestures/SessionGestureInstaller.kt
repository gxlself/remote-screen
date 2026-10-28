package ai.gxlself.remotescreen.rsnative.session.gestures

import ai.gxlself.remotescreen.rsnative.session.RemoteSessionView
import ai.gxlself.remotescreen.rsnative.session.zoom.TouchZoomInstaller

object SessionGestureInstaller {
  fun install(view: RemoteSessionView) {
    val controller = SessionGestureController(view)
    view.gestureLayer = controller
    view.isFocusableInTouchMode = true
    view.setOnFocusChangeListener { _, hasFocus -> controller.onWindowFocusChanged(hasFocus) }
    view.viewTreeObserver.addOnWindowFocusChangeListener { hasFocus ->
      controller.onWindowFocusChanged(hasFocus)
    }
    TouchZoomInstaller.install(controller)
  }
}
