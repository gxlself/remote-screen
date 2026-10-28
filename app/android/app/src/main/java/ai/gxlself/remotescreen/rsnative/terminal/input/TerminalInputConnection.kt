package ai.gxlself.remotescreen.rsnative.terminal.input

import ai.gxlself.remotescreen.rsnative.terminal.RsTerminalView
import android.text.Editable
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection

/** IME connection with a temporary composing buffer and terminal byte output. */
class TerminalInputConnection(
  view: RsTerminalView,
  private val controller: TerminalInputController,
) : BaseInputConnection(view, true) {
  override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean =
    super.setComposingText(text ?: "", newCursorPosition)

  override fun finishComposingText(): Boolean {
    val composing = getEditable()?.let(::composingText).orEmpty()
    val result = super.finishComposingText()
    if (composing.isNotEmpty()) controller.sendCommittedText(composing)
    clearBuffer()
    return result
  }

  override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
    val value = text?.toString().orEmpty()
    val result = super.commitText(text ?: "", newCursorPosition)
    if (value.isNotEmpty()) controller.sendCommittedText(value)
    clearBuffer()
    return result
  }

  override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
    val editable = getEditable()
    if (editable == null || editable.isEmpty()) {
      controller.sendBackspace()
      return true
    }
    return super.deleteSurroundingText(beforeLength, afterLength)
  }

  override fun sendKeyEvent(event: KeyEvent): Boolean = controller.handleInputKeyEvent(event)

  override fun performEditorAction(actionCode: Int): Boolean {
    controller.sendEditorEnter()
    clearBuffer()
    return true
  }

  private fun clearBuffer() {
    getEditable()?.let {
      BaseInputConnection.removeComposingSpans(it)
      it.clear()
    }
  }

  private fun composingText(editable: Editable): String {
    val start = BaseInputConnection.getComposingSpanStart(editable)
    val end = BaseInputConnection.getComposingSpanEnd(editable)
    return if (start >= 0 && end >= start && end <= editable.length) {
      editable.subSequence(start, end).toString()
    } else {
      ""
    }
  }
}
