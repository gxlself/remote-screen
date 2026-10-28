package ai.gxlself.remotescreen.rsnative.terminal

import com.termux.terminal.TerminalEmulator
import com.termux.terminal.TerminalOutput
import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient

/** Bridges emulator output (including device query responses) to the input sender. */
class RsTerminalOutput(
  private val sender: TerminalInputSender,
  private val copyToClipboard: ((String) -> Unit)? = null,
) : TerminalOutput() {
  override fun write(data: ByteArray, offset: Int, count: Int) {
    if (count <= 0) return
    sender.sendRaw(data.copyOfRange(offset, offset + count))
  }

  override fun titleChanged(oldTitle: String?, newTitle: String?) = Unit
  override fun onBell() = Unit
  override fun onColorsChanged() = Unit
  override fun onCopyTextToClipboard(text: String?) {
    if (text != null) copyToClipboard?.invoke(text)
  }
  override fun onPasteTextFromClipboard() = Unit
}

/** The emulator does not use a TerminalSession, so session callbacks are intentionally empty. */
object NoopSessionClient : TerminalSessionClient {
  override fun onTextChanged(changedSession: TerminalSession?) = Unit
  override fun onTitleChanged(changedSession: TerminalSession?) = Unit
  override fun onSessionFinished(finishedSession: TerminalSession?) = Unit
  override fun onCopyTextToClipboard(session: TerminalSession?, text: String?) = Unit
  override fun onPasteTextFromClipboard(session: TerminalSession?) = Unit
  override fun onBell(session: TerminalSession?) = Unit
  override fun onColorsChanged(session: TerminalSession?) = Unit
  override fun onTerminalCursorStateChange(state: Boolean) = Unit
  override fun getTerminalCursorStyle(): Int = TerminalEmulator.DEFAULT_TERMINAL_CURSOR_STYLE
  override fun logError(tag: String?, message: String?) = Unit
  override fun logWarn(tag: String?, message: String?) = Unit
  override fun logInfo(tag: String?, message: String?) = Unit
  override fun logDebug(tag: String?, message: String?) = Unit
  override fun logVerbose(tag: String?, message: String?) = Unit
  override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) = Unit
  override fun logStackTrace(tag: String?, e: Exception?) = Unit
}
