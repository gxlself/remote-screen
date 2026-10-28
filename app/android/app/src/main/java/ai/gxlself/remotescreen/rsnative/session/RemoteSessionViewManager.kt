package ai.gxlself.remotescreen.rsnative.session

import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import ai.gxlself.remotescreen.rsnative.session.gestures.SessionGestureInstaller
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.uimanager.SimpleViewManager
import com.facebook.react.uimanager.ThemedReactContext
import com.facebook.react.uimanager.ViewManagerDelegate
import com.facebook.react.viewmanagers.RemoteSessionViewManagerDelegate
import com.facebook.react.viewmanagers.RemoteSessionViewManagerInterface

@ReactModule(name = RemoteSessionViewManager.NAME)
class RemoteSessionViewManager : SimpleViewManager<RemoteSessionView>(),
  RemoteSessionViewManagerInterface<RemoteSessionView> {
  private val delegate: ViewManagerDelegate<RemoteSessionView> =
    RemoteSessionViewManagerDelegate<RemoteSessionView, RemoteSessionViewManager>(this)

  override fun getName(): String = NAME

  override fun createViewInstance(reactContext: ThemedReactContext): RemoteSessionView =
    RemoteSessionView(reactContext).also(SessionGestureInstaller::install)

  override fun getDelegate(): ViewManagerDelegate<RemoteSessionView> = delegate

  override fun setSessionId(view: RemoteSessionView, value: String?) {
    view.setSessionId(value)
  }

  override fun setMode(view: RemoteSessionView, value: String?) {
    view.setMode(value)
  }

  override fun setKeyboardInset(view: RemoteSessionView, value: Float) {
    view.gestureLayer?.onKeyboardInset(value * view.resources.displayMetrics.density)
  }

  override fun setToolbarCollapsed(view: RemoteSessionView, value: Boolean) {
    view.toolbarCollapsed = value
  }

  override fun setReduceMotion(view: RemoteSessionView, value: Boolean) {
    view.reduceMotion = value
  }

  override fun setCursorSpeed(view: RemoteSessionView, value: Float) {
    view.cursorSpeed = value
  }

  override fun setNaturalScroll(view: RemoteSessionView, value: Boolean) {
    view.naturalScroll = value
  }

  override fun setHaptics(view: RemoteSessionView, value: Boolean) {
    view.haptics = value
  }

  override fun setSpaceIds(view: RemoteSessionView, value: ReadableArray?) {
    view.spaceIds = value.strings()
  }

  override fun setSpaceNames(view: RemoteSessionView, value: ReadableArray?) {
    view.spaceNames = value.strings()
  }

  // The last-picture placeholder is iOS only; Android shows its usual wait.
  override fun setShowSnapshot(view: RemoteSessionView, value: Boolean) = Unit

  override fun animateSpace(view: RemoteSessionView, sessionId: String?, direction: String?) {
    // No slide on Android yet: switch at once.
    if (!sessionId.isNullOrEmpty()) view.emitSpaceSwitch(sessionId)
  }

  private fun ReadableArray?.strings(): List<String> {
    val array = this ?: return emptyList()
    return (0 until array.size()).mapNotNull { array.getString(it) }
  }

  override fun zoomReset(view: RemoteSessionView) {
    dispatchCommand(view, "zoomReset", null)
  }

  override fun centerCursor(view: RemoteSessionView) {
    dispatchCommand(view, "centerCursor", null)
  }

  override fun showMagnifier(view: RemoteSessionView, on: Boolean) {
    val args = com.facebook.react.bridge.Arguments.createArray()
    args.pushBoolean(on)
    dispatchCommand(view, "showMagnifier", args)
  }

  private fun dispatchCommand(view: RemoteSessionView, name: String, args: ReadableArray?) {
    view.dispatchCommand(name, args)
  }

  override fun getExportedCustomDirectEventTypeConstants(): MutableMap<String, Any> =
    RsViewEvents.directEventTypes(
      "topGesture",
      "topViewport",
      "topFirstFrame",
      "topVideoStats",
      "topSpaceSwipe",
      "topSpaceSwitch",
    )

  override fun onDropViewInstance(view: RemoteSessionView) {
    view.gestureLayer?.onDetached()
    view.detachSession()
    super.onDropViewInstance(view)
  }

  companion object {
    const val NAME = "RemoteSessionView"
  }
}
