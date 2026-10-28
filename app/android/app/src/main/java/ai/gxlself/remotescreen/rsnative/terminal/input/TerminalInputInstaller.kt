package ai.gxlself.remotescreen.rsnative.terminal.input

import ai.gxlself.remotescreen.rsnative.terminal.RsTerminalView
import java.lang.ref.WeakReference
import java.util.WeakHashMap

/** Installs the terminal input implementation on an R6B terminal view. */
object TerminalInputInstaller {
  private val controllers = WeakHashMap<RsTerminalView, WeakReference<TerminalInputController>>()

  /** Commands are global in the Fabric manager, so route each command by its view. */
  var copySelection: ((RsTerminalView) -> Unit)? = null
  var paste: ((RsTerminalView) -> Unit)? = null

  fun install(view: RsTerminalView) {
    val controller = TerminalInputController(view)
    controllers[view] = WeakReference(controller)
    view.inputDelegate = controller
    view.clipboardWriter = controller::writeClipboard
    copySelection = { target -> controllers[target]?.get()?.copySelection() }
    paste = { target -> controllers[target]?.get()?.paste() }
  }
}
