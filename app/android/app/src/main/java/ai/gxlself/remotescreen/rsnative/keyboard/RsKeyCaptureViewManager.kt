package ai.gxlself.remotescreen.rsnative.keyboard

import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import com.facebook.react.uimanager.SimpleViewManager
import com.facebook.react.uimanager.ThemedReactContext
import com.facebook.react.uimanager.ViewManagerDelegate
import com.facebook.react.viewmanagers.RsKeyCaptureManagerDelegate
import com.facebook.react.viewmanagers.RsKeyCaptureManagerInterface

class RsKeyCaptureViewManager :
  SimpleViewManager<RsKeyCaptureView>(),
  RsKeyCaptureManagerInterface<RsKeyCaptureView> {
  private val delegate = RsKeyCaptureManagerDelegate<RsKeyCaptureView, RsKeyCaptureViewManager>(this)

  override fun getName(): String = NAME

  override fun createViewInstance(context: ThemedReactContext): RsKeyCaptureView =
    RsKeyCaptureView(context)

  override fun getDelegate(): ViewManagerDelegate<RsKeyCaptureView> = delegate

  override fun setActive(view: RsKeyCaptureView, value: Boolean) {
    view.setActive(value)
  }

  override fun setInputMode(view: RsKeyCaptureView, value: String?) {
    view.setInputMode(value)
  }

  override fun setStickyMods(view: RsKeyCaptureView, value: Int) {
    view.setStickyMods(value)
  }

  override fun setOs(view: RsKeyCaptureView, value: String?) {
    view.setOs(value)
  }

  // Android hardware Ctrl+V goes straight out as keys; only iOS waits for JS
  // to push the clipboard first (onPaste is never sent here).
  override fun setSyncPaste(view: RsKeyCaptureView, value: Boolean) = Unit

  override fun getExportedCustomDirectEventTypeConstants(): MutableMap<String, Any> =
    RsViewEvents.directEventTypes(
      "topKey",
      "topText",
      "topComposition",
      "topStickyConsumed",
      "topKeyboardFrame",
      "topActiveChange",
      "topPaste",
      "topShortcut",
    )

  companion object {
    const val NAME = "RsKeyCapture"
  }
}
