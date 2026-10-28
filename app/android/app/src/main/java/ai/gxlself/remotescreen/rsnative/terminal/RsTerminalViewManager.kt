package ai.gxlself.remotescreen.rsnative.terminal

import com.facebook.react.uimanager.SimpleViewManager
import com.facebook.react.uimanager.ThemedReactContext
import com.facebook.react.uimanager.ViewManagerDelegate
import com.facebook.react.viewmanagers.RsTerminalViewManagerDelegate
import com.facebook.react.viewmanagers.RsTerminalViewManagerInterface
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import ai.gxlself.remotescreen.rsnative.terminal.input.TerminalInputInstaller

class RsTerminalViewManager :
  SimpleViewManager<RsTerminalView>(),
  RsTerminalViewManagerInterface<RsTerminalView> {
  private val delegate: ViewManagerDelegate<RsTerminalView> = RsTerminalViewManagerDelegate(this)

  override fun getName(): String = NAME

  override fun getDelegate(): ViewManagerDelegate<RsTerminalView> = delegate

  override fun createViewInstance(reactContext: ThemedReactContext): RsTerminalView {
    return RsTerminalView(reactContext).also { TerminalInputInstaller.install(it) }
  }

  override fun setSshId(view: RsTerminalView, value: String?) {
    view.setSshId(value)
  }

  override fun setFontSize(view: RsTerminalView, value: Float) {
    view.setFontSize(value)
  }

  override fun setTheme(view: RsTerminalView, value: String?) {
    view.setTheme(value)
  }

  override fun setModifiers(view: RsTerminalView, value: Int) {
    view.modifiers = value
  }

  override fun setKeyboardActive(view: RsTerminalView, value: Boolean) {
    view.keyboardActive = value
  }

  override fun setDebugEcho(view: RsTerminalView, value: Boolean) {
    view.setDebugEcho(value)
  }

  override fun sendSequence(view: RsTerminalView, text: String) {
    view.sendSequence(text)
  }

  override fun sendKey(view: RsTerminalView, name: String) {
    view.sendKey(name)
  }

  override fun copySelection(view: RsTerminalView) {
    view.copySelectionCommand()
  }

  override fun paste(view: RsTerminalView) {
    view.pasteCommand()
  }

  override fun getExportedCustomDirectEventTypeConstants(): MutableMap<String, Any> =
    RsViewEvents.directEventTypes(
      "topModifiersConsumed",
      "topSizeChange",
      "topKeyboardFrame",
      "topActiveChange",
      "topCopied",
    )

  companion object {
    const val NAME = "RsTerminalView"
  }
}
