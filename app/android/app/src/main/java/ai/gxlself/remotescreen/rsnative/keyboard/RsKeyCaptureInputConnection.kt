package ai.gxlself.remotescreen.rsnative.keyboard

import android.text.Editable
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo

/** Input connection whose editable is only a temporary IME composition buffer. */
class RsKeyCaptureInputConnection(private val capture: RsKeyCaptureView) :
  BaseInputConnection(capture, true) {
  private var suppressEnterUp = false

  override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
    val result = super.setComposingText(text ?: "", newCursorPosition)
    capture.emitComposition(text?.toString() ?: "")
    return result
  }

  override fun finishComposingText(): Boolean {
    val editable = getEditable()
    val composing = editable?.let { composingText(it) }.orEmpty()
    val result = super.finishComposingText()
    if (composing.isNotEmpty()) {
      capture.commitText(composing)
    }
    clearBuffer()
    capture.emitComposition("")
    return result
  }

  override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
    val value = text?.toString().orEmpty()
    val result = super.commitText(text ?: "", newCursorPosition)
    if (value.isNotEmpty()) capture.commitText(value)
    clearBuffer()
    capture.emitComposition("")
    return result
  }

  override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
    val editable = getEditable()
    val hasComposing = editable != null && composingText(editable).isNotEmpty()
    if (!hasComposing && editable?.isEmpty() != false) {
      capture.sendBackspace()
      return true
    }
    val result = super.deleteSurroundingText(beforeLength, afterLength)
    getEditable()?.let { updated ->
      capture.emitComposition(composingText(updated))
    }
    return result
  }

  override fun sendKeyEvent(event: KeyEvent): Boolean {
    val composing = getEditable()?.let { composingText(it) }.orEmpty().isNotEmpty()
    if (event.keyCode == KeyEvent.KEYCODE_ENTER) {
      if (composing) {
        // IMEs use Enter here to accept the current candidate. The committed
        // candidate arrives through commitText and must not produce an extra HID Enter.
        suppressEnterUp = true
        return true
      }
      if (suppressEnterUp && event.action == KeyEvent.ACTION_UP) {
        suppressEnterUp = false
        return true
      }
    }
    if (event.keyCode == KeyEvent.KEYCODE_DEL && composing) {
      if (event.action == KeyEvent.ACTION_DOWN) deleteSurroundingText(1, 0)
      return true
    }
    return capture.handleKeyEvent(event)
  }

  override fun performEditorAction(actionCode: Int): Boolean {
    capture.sendEnter()
    return true
  }

  override fun closeConnection() {
    super.closeConnection()
  }

  private fun clearBuffer() {
    getEditable()?.let { editable ->
      BaseInputConnection.removeComposingSpans(editable)
      editable.clear()
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
