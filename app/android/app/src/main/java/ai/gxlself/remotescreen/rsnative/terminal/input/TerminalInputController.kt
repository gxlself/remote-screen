package ai.gxlself.remotescreen.rsnative.terminal.input

import ai.gxlself.remotescreen.logic.terminal.TermHardwareKeys
import ai.gxlself.remotescreen.logic.terminal.TermModifierApply
import ai.gxlself.remotescreen.logic.terminal.TermMods
import ai.gxlself.remotescreen.logic.terminal.TermPasteEncoder
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.keyboard.KeyboardFrameObserver
import ai.gxlself.remotescreen.rsnative.terminal.RsTerminalView
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.text.InputType
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.termux.terminal.TerminalBuffer
import com.termux.terminal.TerminalEmulator
import kotlin.math.abs

/** Android keyboard, hardware-key and terminal-selection implementation. */
class TerminalInputController(private val view: RsTerminalView) : RsTerminalView.TerminalInputDelegate {
  private val frameObserver = KeyboardFrameObserver(view) { height, visible, hardware ->
    view.emitKeyboardFrame(height, visible, hardware)
  }
  private val clipboard: ClipboardManager? =
    view.context.getSystemService(ClipboardManager::class.java)
  private val longPressTimeout = ViewConfiguration.getLongPressTimeout().toLong()
  private val touchSlop = ViewConfiguration.get(view.context).scaledTouchSlop
  private val handlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
    color = HANDLE_COLOR
    style = Paint.Style.FILL
  }
  private var lastActive: Boolean? = null
  private var lastKeyboardRequest: Boolean? = null
  private var longPressRunnable: Runnable? = null
  private var downX = 0f
  private var downY = 0f
  private var downColumn = 0
  private var downRow = 0
  private var longPressed = false
  private var selecting = false
  private var draggedHandle = Handle.NONE
  private val handledUnicodeKeys = HashSet<Int>()

  init {
    lastKeyboardRequest = view.keyboardActive
    view.setOnFocusChangeListener { _, active -> publishActive(active) }
    view.addOnAttachStateChangeListener(object : View.OnAttachStateChangeListener {
      override fun onViewAttachedToWindow(v: View) {
        frameObserver.start()
        if (view.keyboardActive) {
          view.requestFocus()
          view.post { if (view.keyboardActive && view.isAttachedToWindow) showKeyboard() }
        }
      }

      override fun onViewDetachedFromWindow(v: View) {
        frameObserver.stop()
      }
    })
    if (view.isAttachedToWindow) frameObserver.start()
  }

  override fun onCreateInputConnection(
    view: RsTerminalView,
    outAttrs: EditorInfo,
  ): InputConnection {
    outAttrs.inputType =
      InputType.TYPE_CLASS_TEXT or
        InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
        InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
    outAttrs.imeOptions =
      EditorInfo.IME_FLAG_NO_EXTRACT_UI or
        EditorInfo.IME_FLAG_NO_FULLSCREEN or
        EditorInfo.IME_ACTION_NONE
    return TerminalInputConnection(view, this)
  }

  override fun onKeyDown(view: RsTerminalView, keyCode: Int, event: KeyEvent): Boolean {
    val mods = TermMods.fromAndroidMeta(event.metaState) or view.modifiers
    val appCursor = view.emulator?.isCursorKeysApplicationMode == true
    val hardware = TermHardwareKeys.bytes(keyCode, appCursor, mods)
    if (hardware != null) {
      val repeats = (event.repeatCount + 1).coerceAtLeast(1)
      repeat(repeats) { view.inputSender.sendRaw(hardware) }
      consumeStickyModifiers(view, view.modifiers)
      return true
    }

    if (event.isSystem) return false

    val unicode = event.unicodeChar
    if (unicode != 0) {
      val repeats = (event.repeatCount + 1).coerceAtLeast(1)
      repeat(repeats) { sendCodePoint(view, unicode, mods, consumeSticky = it == 0) }
      handledUnicodeKeys += keyCode
      return true
    }
    // Volume, BACK, HOME and other system keys belong to Android.
    return false
  }

  override fun onKeyUp(view: RsTerminalView, keyCode: Int, event: KeyEvent): Boolean {
    val mods = TermMods.fromAndroidMeta(event.metaState) or view.modifiers
    if (TermHardwareKeys.bytes(keyCode, view.emulator?.isCursorKeysApplicationMode == true, mods) != null) {
      return true
    }
    if (event.isSystem) return false
    return handledUnicodeKeys.remove(keyCode)
  }

  override fun onTouchEvent(view: RsTerminalView, event: MotionEvent): Boolean {
    val action = event.actionMasked
    when (action) {
      MotionEvent.ACTION_DOWN -> {
        downX = event.x
        downY = event.y
        val cell = view.cellAt(event.x, event.y)
        downColumn = cell.first
        downRow = cell.second
        longPressed = false
        selecting = false
        draggedHandle = handleAt(event.x, event.y)
        if (draggedHandle != Handle.NONE) {
          selecting = true
          disallowParent(true)
          return true
        }
        cancelLongPress()
        val runnable = Runnable {
          if (view.isAttachedToWindow && !longPressed) {
            longPressed = true
            val word = wordSelectionAt(view.emulator?.screen, downColumn, downRow)
            if (word != null) {
              selecting = true
              view.selection = word
              disallowParent(true)
              view.invalidate()
            }
          }
        }
        longPressRunnable = runnable
        view.postDelayed(runnable, longPressTimeout)
        // Returning false leaves ordinary one-finger history scrolling to R6B.
        return false
      }

      MotionEvent.ACTION_MOVE -> {
        if (selecting) {
          updateDraggedSelection(view, view.cellAt(event.x, event.y))
          return true
        }
        if (!longPressed && (abs(event.x - downX) > touchSlop || abs(event.y - downY) > touchSlop)) {
          cancelLongPress()
        }
        return false
      }

      MotionEvent.ACTION_UP -> {
        cancelLongPress()
        val consumed = selecting || draggedHandle != Handle.NONE
        if (selecting) {
          updateDraggedSelection(view, view.cellAt(event.x, event.y))
          disallowParent(false)
        } else if (view.selection != null &&
          abs(event.x - downX) <= touchSlop && abs(event.y - downY) <= touchSlop
        ) {
          // A light tap outside a handle clears a previous selection.
          view.selection = null
        }
        selecting = false
        draggedHandle = Handle.NONE
        longPressed = false
        return consumed
      }

      MotionEvent.ACTION_CANCEL -> {
        cancelLongPress()
        val wasSelecting = selecting
        if (wasSelecting) disallowParent(false)
        selecting = false
        draggedHandle = Handle.NONE
        longPressed = false
        return wasSelecting
      }

      MotionEvent.ACTION_POINTER_DOWN -> {
        cancelLongPress()
        return selecting
      }
    }
    return selecting
  }

  override fun onDrawOverlay(view: RsTerminalView, canvas: Canvas) {
    val range = view.selection ?: return
    val emulator = view.emulator ?: return
    val columns = emulator.mColumns.coerceAtLeast(1)
    val rows = emulator.mRows.coerceAtLeast(1)
    val cellWidth = view.width.toFloat() / columns
    val cellHeight = view.height.toFloat() / rows
    drawHandle(canvas, range[0], range[1], cellWidth, cellHeight)
    drawHandle(canvas, range[2], range[3], cellWidth, cellHeight)
  }

  override fun onPropsChanged(view: RsTerminalView) {
    val keyboard = view.keyboardActive
    if (lastKeyboardRequest != keyboard) {
      lastKeyboardRequest = keyboard
      RsLog.marker("TERMVIEW", mapOf("keyboard" to keyboard))
    }
    if (keyboard) {
      view.requestFocus()
      if (view.isAttachedToWindow) {
        view.post { if (view.keyboardActive && view.isAttachedToWindow) showKeyboard() }
      }
    } else {
      hideKeyboard()
      view.clearFocus()
    }
    publishActive(view.hasFocus())
  }

  override fun onDetached(view: RsTerminalView) {
    cancelLongPress()
    frameObserver.stop()
    handledUnicodeKeys.clear()
    if (lastActive == true) publishActive(false)
  }

  fun copySelection() {
    val range = view.selection ?: return
    val text = view.emulator?.screen?.getSelectedText(range[0], range[1], range[2], range[3]).orEmpty()
    if (text.isEmpty()) return
    writeClipboard(text)
    view.selection = null
  }

  fun paste() {
    val text = clipboard?.primaryClip?.let { clip ->
      if (clip.itemCount == 0) null else clip.getItemAt(0).coerceToText(view.context)?.toString()
    }.orEmpty()
    if (text.isEmpty()) return
    val bracketed = view.emulator?.isBracketedPasteMode == true
    view.inputSender.sendRaw(TermPasteEncoder.encode(text, bracketed))
    RsLog.marker("TERMVIEW", mapOf("paste" to text.length))
  }

  /** Shared clipboard writer for both toolbar copy and OSC 52 output. */
  fun writeClipboard(text: String) {
    clipboard?.setPrimaryClip(ClipData.newPlainText("Remote Screen", text))
    view.emitCopied(text.length)
    RsLog.marker("TERMVIEW", mapOf("copy" to text.length))
  }

  internal fun sendCommittedText(text: String) {
    if (text.isEmpty()) return
    val modifiers = view.modifiers
    val output = java.io.ByteArrayOutputStream()
    var first = true
    text.codePoints().forEach { codePoint ->
      val value = if (codePoint == NEWLINE) CARRIAGE_RETURN.toInt() else codePoint
      val bytes = String(Character.toChars(value)).toByteArray(Charsets.UTF_8)
      if (first && modifiers != 0) {
        val applied = TermModifierApply.apply(bytes, modifiers)
        output.write(applied.bytes)
        consumeStickyModifiers(view, modifiers)
      } else {
        output.write(bytes)
      }
      first = false
    }
    view.inputSender.sendRaw(output.toByteArray())
  }

  internal fun sendEditorEnter() {
    view.inputSender.sendRaw(byteArrayOf(CARRIAGE_RETURN))
  }

  internal fun handleInputKeyEvent(event: KeyEvent): Boolean = when (event.action) {
    KeyEvent.ACTION_DOWN -> onKeyDown(view, event.keyCode, event)
    KeyEvent.ACTION_UP -> onKeyUp(view, event.keyCode, event)
    else -> false
  }

  internal fun sendBackspace() {
    val modifiers = view.modifiers
    val applied = TermModifierApply.apply(byteArrayOf(DELETE), modifiers)
    view.inputSender.sendRaw(applied.bytes)
    consumeStickyModifiers(view, modifiers)
  }

  private fun sendCodePoint(
    view: RsTerminalView,
    codePoint: Int,
    mods: Int,
    consumeSticky: Boolean,
  ) {
    val normalized = if (codePoint == NEWLINE) CARRIAGE_RETURN.toInt() else codePoint
    val bytes = String(Character.toChars(normalized)).toByteArray(Charsets.UTF_8)
    val applied = TermModifierApply.apply(bytes, mods)
    view.inputSender.sendRaw(applied.bytes)
    if (consumeSticky) consumeStickyModifiers(view, view.modifiers)
  }

  private fun consumeStickyModifiers(view: RsTerminalView, sticky: Int) {
    if (sticky != 0) view.emitModifiersConsumed(sticky)
  }

  private fun publishActive(active: Boolean) {
    if (lastActive == active) return
    lastActive = active
    view.emitActiveChange(active)
  }

  private fun wordSelectionAt(screen: TerminalBuffer?, column: Int, row: Int): IntArray? {
    if (screen == null) return null
    val columns = view.emulator?.mColumns?.coerceAtLeast(1) ?: return null
    if (screen.getWordAtLocation(column, row).isEmpty()) return null
    val line = screen.getSelectedText(0, row, columns - 1, row, false)
    if (line.isEmpty()) return null
    val index = column.coerceIn(0, line.lastIndex)
    if (line[index].isWhitespace()) return null
    var start = index
    while (start > 0 && !line[start - 1].isWhitespace()) start--
    var end = index
    while (end + 1 < line.length && !line[end + 1].isWhitespace()) end++
    return intArrayOf(start, row, end, row)
  }

  private fun updateDraggedSelection(view: RsTerminalView, cell: Pair<Int, Int>) {
    val range = view.selection ?: return
    val next = when (draggedHandle) {
      Handle.START -> intArrayOf(cell.first, cell.second, range[2], range[3])
      Handle.END -> intArrayOf(range[0], range[1], cell.first, cell.second)
      Handle.NONE -> intArrayOf(range[0], range[1], cell.first, cell.second)
    }
    view.selection = normalizeRange(next)
  }

  private fun normalizeRange(range: IntArray): IntArray {
    val before = range[1] < range[3] || (range[1] == range[3] && range[0] <= range[2])
    return if (before) range else intArrayOf(range[2], range[3], range[0], range[1])
  }

  private fun handleAt(x: Float, y: Float): Handle {
    val range = view.selection ?: return Handle.NONE
    val emulator = view.emulator ?: return Handle.NONE
    val columns = emulator.mColumns.coerceAtLeast(1)
    val rows = emulator.mRows.coerceAtLeast(1)
    val cellWidth = view.width.toFloat() / columns
    val cellHeight = view.height.toFloat() / rows
    val radius = 12f * view.resources.displayMetrics.density
    val start = handleCenter(range[0], range[1], cellWidth, cellHeight)
    val end = handleCenter(range[2], range[3], cellWidth, cellHeight)
    return when {
      distanceSquared(x, y, start.first, start.second) <= radius * radius -> Handle.START
      distanceSquared(x, y, end.first, end.second) <= radius * radius -> Handle.END
      else -> Handle.NONE
    }
  }

  private fun drawHandle(canvas: Canvas, column: Int, row: Int, cellWidth: Float, cellHeight: Float) {
    val center = handleCenter(column, row, cellWidth, cellHeight)
    val radius = 8f * view.resources.displayMetrics.density
    if (center.second + radius >= 0f && center.second - radius <= view.height) {
      canvas.drawCircle(center.first, center.second, radius, handlePaint)
    }
  }

  private fun handleCenter(column: Int, row: Int, cellWidth: Float, cellHeight: Float): Pair<Float, Float> =
    (column + 0.5f) * cellWidth to (row - view.topRow + 0.5f) * cellHeight

  private fun distanceSquared(x1: Float, y1: Float, x2: Float, y2: Float): Float {
    val dx = x1 - x2
    val dy = y1 - y2
    return dx * dx + dy * dy
  }

  private fun cancelLongPress() {
    longPressRunnable?.let(view::removeCallbacks)
    longPressRunnable = null
  }

  private fun disallowParent(disallow: Boolean) {
    view.parent?.requestDisallowInterceptTouchEvent(disallow)
  }

  private fun showKeyboard() {
    if (android.os.Build.VERSION.SDK_INT >= 30) {
      ViewCompat.getWindowInsetsController(view)?.show(WindowInsetsCompat.Type.ime())
    } else {
      val manager = view.context.getSystemService(InputMethodManager::class.java)
      manager?.showSoftInput(view, InputMethodManager.SHOW_IMPLICIT)
    }
  }

  private fun hideKeyboard() {
    if (android.os.Build.VERSION.SDK_INT >= 30) {
      ViewCompat.getWindowInsetsController(view)?.hide(WindowInsetsCompat.Type.ime())
    } else {
      val manager = view.context.getSystemService(InputMethodManager::class.java)
      manager?.hideSoftInputFromWindow(view.windowToken, 0)
    }
  }

  private enum class Handle { NONE, START, END }

  private companion object {
    const val HANDLE_COLOR = Color.CYAN
    const val NEWLINE = 0x0a
    const val CARRIAGE_RETURN: Byte = 0x0d
    const val DELETE: Byte = 0x7f
  }
}

/** Termux v0.118.3 keeps bracketed-paste state private; newer releases expose this member. */
private val TerminalEmulator.isBracketedPasteMode: Boolean
  get() = runCatching {
    val flags = javaClass.getDeclaredField("mCurrentDecSetFlags").apply { isAccessible = true }.getInt(this)
    flags and 1024 != 0 // DECSET 2004, as mapped by TerminalEmulator.
  }.getOrDefault(false)
