package ai.gxlself.remotescreen.rsnative.terminal

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.view.inputmethod.EditorInfo
import android.view.KeyEvent
import android.view.MotionEvent
import android.widget.OverScroller
import android.view.VelocityTracker
import android.view.View
import android.view.inputmethod.InputConnection
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import com.facebook.react.bridge.Arguments
import com.termux.terminal.TerminalEmulator
import com.termux.terminal.TextStyle
import com.termux.view.TerminalRenderer
import ai.gxlself.remotescreen.logic.terminal.TermGrid
import ai.gxlself.remotescreen.logic.terminal.TermTheme
import ai.gxlself.remotescreen.logic.terminal.TermFontSize
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents

/** Fabric view which renders a Termux emulator without creating a TerminalSession. */
class RsTerminalView(context: Context) : View(context) {
  interface TerminalInputDelegate {
    fun onCreateInputConnection(view: RsTerminalView, outAttrs: EditorInfo): InputConnection?
    fun onKeyDown(view: RsTerminalView, keyCode: Int, event: KeyEvent): Boolean
    fun onKeyUp(view: RsTerminalView, keyCode: Int, event: KeyEvent): Boolean
    /** Return true when the event was consumed by input/selection handling. */
    fun onTouchEvent(view: RsTerminalView, event: MotionEvent): Boolean
    fun onDrawOverlay(view: RsTerminalView, canvas: Canvas)
    fun onPropsChanged(view: RsTerminalView) = Unit
    fun onDetached(view: RsTerminalView) = Unit
  }

  private val mainHandler = Handler(Looper.getMainLooper())
  private val scroller = OverScroller(context)
  private var velocityTracker: VelocityTracker? = null
  private var lastTouchY = 0f
  private var dragging = false

  private var fontSizeSp = 13f
  private var themeName = "night"
  private var sshIdValue = ""
  private var debugEchoValue = false
  private var attachedId: Long? = null
  private var renderer: TerminalRenderer
  private var terminal: TerminalEmulator? = null
  private var columns = 1
  private var rows = 1
  private var cellWidth = 1f
  private var lineHeight = 1
  private var topRowValue = 0

  private val terminalOutput: RsTerminalOutput
  val inputSender: TerminalInputSender
  private val attachment: TerminalAttachment

  var inputDelegate: TerminalInputDelegate? = null
    set(value) {
      field = value
      invalidate()
    }

  /** The currently parsed terminal, or null before the first layout. */
  val emulator: TerminalEmulator?
    get() = terminal

  var topRow: Int
    get() = topRowValue
    set(value) {
      clampTopRow(value)
    }

  var selection: IntArray? = null
    set(value) {
      field = value?.takeIf { it.size >= 4 }?.copyOf(4)
      invalidate()
    }

  var modifiers: Int = 0
    set(value) {
      if (field == value) return
      field = value
      inputDelegate?.onPropsChanged(this)
    }

  var keyboardActive: Boolean = false
    set(value) {
      if (field == value) return
      field = value
      inputDelegate?.onPropsChanged(this)
    }

  /** Optional clipboard hook installed by the input extension. */
  var clipboardWriter: ((String) -> Unit)? = null

  init {
    isFocusable = true
    isFocusableInTouchMode = true
    setBackgroundColor(Color.BLACK)
    renderer = createRenderer()
    inputSender = TerminalInputSender(this)
    terminalOutput = RsTerminalOutput(inputSender) { text -> onCopyTextToClipboard(text) }
    attachment = TerminalAttachment(this)
    // Keep a parser available before the first layout/attach callback.
    updateGeometry(force = true)
  }

  fun setSshId(value: String?) {
    val next = value ?: ""
    if (sshIdValue == next) return
    sshIdValue = next
    attachedId = null
    inputSender.setDestination(null, false)
    resetTerminal()
    attachment.attach(next)
    syncInputDestination()
  }

  fun setFontSize(value: Float) {
    val next = TermFontSize.clamp(value.toDouble()).toFloat()
    if (fontSizeSp == next) return
    fontSizeSp = next
    renderer = createRenderer()
    updateGeometry(force = true)
    invalidate()
  }

  fun setTheme(value: String?) {
    val next = value ?: "night"
    if (themeName == next) return
    themeName = next
    applyTheme()
    invalidate()
  }

  fun setDebugEcho(value: Boolean) {
    if (debugEchoValue == value) return
    debugEchoValue = value
    syncInputDestination()
  }

  fun sendSequence(text: String?) {
    inputSender.sendSequence(text ?: "")
  }

  fun sendKey(name: String?) {
    inputSender.sendKey(name ?: "")
  }

  fun copySelectionCommand() {
    val action = ai.gxlself.remotescreen.rsnative.terminal.input.TerminalInputInstaller.copySelection
    if (action != null) action(this)
    else emitCommandUnavailable("copySelection")
  }

  fun pasteCommand() {
    val action = ai.gxlself.remotescreen.rsnative.terminal.input.TerminalInputInstaller.paste
    if (action != null) action(this)
    else emitCommandUnavailable("paste")
  }

  /** Called by TerminalOutput for OSC 52 clipboard requests. */
  fun onCopyTextToClipboard(text: String) {
    clipboardWriter?.invoke(text)
  }

  fun emitModifiersConsumed(mods: Int) {
    val map = Arguments.createMap()
    map.putInt("mods", mods)
    RsViewEvents.emit(this, "topModifiersConsumed", map)
  }

  fun emitCopied(length: Int) {
    val map = Arguments.createMap()
    map.putInt("length", length)
    RsViewEvents.emit(this, "topCopied", map)
  }

  fun emitKeyboardFrame(height: Float, visible: Boolean, hardware: Boolean) {
    val map = Arguments.createMap()
    map.putDouble("height", height.toDouble())
    map.putBoolean("visible", visible)
    map.putBoolean("hardware", hardware)
    RsViewEvents.emit(this, "topKeyboardFrame", map)
  }

  fun emitActiveChange(active: Boolean) {
    val map = Arguments.createMap()
    map.putBoolean("active", active)
    RsViewEvents.emit(this, "topActiveChange", map)
  }

  /** Returns the terminal column and external terminal row at a pixel position. */
  fun cellAt(xPx: Float, yPx: Float): Pair<Int, Int> {
    val column = if (cellWidth > 0f) (xPx / cellWidth).toInt() else 0
    val row = if (lineHeight > 0) (yPx / lineHeight).toInt() + topRowValue else topRowValue
    val maxColumn = (terminal?.mColumns ?: columns).coerceAtLeast(1) - 1
    val maxRow = (terminal?.mRows ?: rows).coerceAtLeast(1) - 1
    return column.coerceIn(0, maxColumn) to row.coerceIn(-activeTranscriptRows(), maxRow)
  }

  override fun onCheckIsTextEditor(): Boolean = inputDelegate != null

  override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? =
    inputDelegate?.onCreateInputConnection(this, outAttrs)

  override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
    return inputDelegate?.onKeyDown(this, keyCode, event) == true || super.onKeyDown(keyCode, event)
  }

  override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
    return inputDelegate?.onKeyUp(this, keyCode, event) == true || super.onKeyUp(keyCode, event)
  }

  override fun onSizeChanged(width: Int, height: Int, oldWidth: Int, oldHeight: Int) {
    super.onSizeChanged(width, height, oldWidth, oldHeight)
    updateGeometry(force = false)
  }

  override fun onDraw(canvas: Canvas) {
    super.onDraw(canvas)
    val current = terminal
    if (current != null) {
      val range = selection
      val x1 = range?.getOrNull(0) ?: -1
      val y1 = range?.getOrNull(1) ?: -1
      val x2 = range?.getOrNull(2) ?: -1
      val y2 = range?.getOrNull(3) ?: -1
      renderer.render(current, canvas, topRowValue, y1, y2, x1, x2)
    }
    inputDelegate?.onDrawOverlay(this, canvas)
  }

  override fun onTouchEvent(event: MotionEvent): Boolean {
    if (inputDelegate?.onTouchEvent(this, event) == true) return true
    if (terminal?.isAlternateBufferActive == true) {
      topRow = 0
      return true
    }
    // History scrolling is deliberately a single-finger gesture. Leave
    // multi-touch available to the R6C input/selection extension.
    if (event.pointerCount > 1 || event.actionMasked == MotionEvent.ACTION_POINTER_DOWN) {
      if (dragging) finishTouch()
      return true
    }

    when (event.actionMasked) {
      MotionEvent.ACTION_DOWN -> {
        parent?.requestDisallowInterceptTouchEvent(true)
        scroller.abortAnimation()
        velocityTracker?.recycle()
        velocityTracker = VelocityTracker.obtain().also { it.addMovement(event) }
        lastTouchY = event.y
        dragging = true
        return true
      }
      MotionEvent.ACTION_MOVE -> {
        velocityTracker?.addMovement(event)
        if (!dragging) return true
        val delta = lastTouchY - event.y
        if (kotlin.math.abs(delta) >= 1f) {
          val line = lineHeight.coerceAtLeast(1)
          val rowsDelta = (delta / line).toInt()
          if (rowsDelta != 0) {
            topRow = topRowValue - rowsDelta
            lastTouchY -= rowsDelta * line
          }
        }
        return true
      }
      MotionEvent.ACTION_UP -> {
        velocityTracker?.addMovement(event)
        velocityTracker?.computeCurrentVelocity(1000)
        val velocityY = velocityTracker?.yVelocity ?: 0f
        val maxScroll = activeTranscriptRows() * lineHeight.coerceAtLeast(1)
        if (maxScroll > 0 && kotlin.math.abs(velocityY) > 80f) {
          val currentOffset = -topRowValue * lineHeight.coerceAtLeast(1)
          scroller.fling(0, currentOffset, 0, -velocityY.toInt(), 0, 0, 0, maxScroll)
          postInvalidateOnAnimation()
        }
        finishTouch()
        return true
      }
      MotionEvent.ACTION_CANCEL -> {
        finishTouch()
        return true
      }
    }
    return true
  }

  override fun computeScroll() {
    super.computeScroll()
    if (scroller.computeScrollOffset()) {
      val line = lineHeight.coerceAtLeast(1)
      topRow = -(scroller.currY / line)
      postInvalidateOnAnimation()
    }
  }

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    if (sshIdValue.isNotEmpty()) attachment.attach(sshIdValue)
    syncInputDestination()
  }

  override fun onDetachedFromWindow() {
    attachment.detach()
    attachedId = null
    inputSender.setDestination(null, false)
    inputDelegate?.onDetached(this)
    super.onDetachedFromWindow()
  }

  fun feedOutput(data: ByteArray) {
    if (data.isEmpty()) return
    val wasAtBottom = topRowValue == 0
    terminal?.append(data, data.size)
    if (wasAtBottom) topRowValue = 0 else clampTopRow(topRowValue)
    invalidate()
  }

  fun feedLocal(data: ByteArray) {
    feedOutput(data)
  }

  internal fun onTerminalAttached(id: Long) {
    attachedId = id
    syncInputDestination()
    attachment.resize(columns, rows)
  }

  internal fun onTerminalDetached() {
    attachedId = null
    syncInputDestination()
  }


  internal fun postToMain(action: () -> Unit) {
    if (Looper.myLooper() == Looper.getMainLooper()) action() else mainHandler.post(action)
  }

  private fun createRenderer(): TerminalRenderer {
    val textSizePx = TypedValue.applyDimension(
      TypedValue.COMPLEX_UNIT_SP,
      TermFontSize.clamp(fontSizeSp.toDouble()).toFloat(),
      resources.displayMetrics,
    ).coerceAtLeast(1f).toInt()
    return TerminalRenderer(textSizePx, Typeface.MONOSPACE)
  }

  private fun updateGeometry(force: Boolean) {
    val rendererCellWidth = renderer.fontWidthSafe()
    val rendererLineHeight = renderer.fontLineSpacingSafe()
    val padding = TypedValue.applyDimension(
      TypedValue.COMPLEX_UNIT_DIP,
      4f,
      resources.displayMetrics,
    ).toInt().coerceAtLeast(0)
    val nextGrid = TermGrid.size(width, height, rendererCellWidth, rendererLineHeight, padding)
    val nextCellWidth = rendererCellWidth.coerceAtLeast(1f)
    val nextLineHeight = rendererLineHeight.coerceAtLeast(1)
    val changed = force || nextGrid.first != columns || nextGrid.second != rows ||
      nextCellWidth != cellWidth || nextLineHeight != lineHeight
    cellWidth = nextCellWidth
    lineHeight = nextLineHeight
    if (!changed) return
    columns = nextGrid.first
    rows = nextGrid.second
    val current = terminal
    if (current == null) {
      terminal = TerminalEmulator(
        terminalOutput,
        columns,
        rows,
        kotlin.math.ceil(cellWidth.toDouble()).toInt().coerceAtLeast(1),
        lineHeight,
        2000,
        NoopSessionClient,
      )
      applyTheme()
    } else {
      current.resize(
        columns,
        rows,
        kotlin.math.ceil(cellWidth.toDouble()).toInt().coerceAtLeast(1),
        lineHeight,
      )
      applyTheme()
    }
    clampTopRow(topRowValue)
    attachment.resize(columns, rows)
    val map = Arguments.createMap()
    map.putInt("cols", columns)
    map.putInt("rows", rows)
    RsViewEvents.emit(this, "topSizeChange", map)
    invalidate()
  }

  private fun TerminalRenderer.fontWidthSafe(): Float = getFontWidth().takeIf { it.isFinite() && it > 0f } ?: run {
    val paint = Paint().apply {
      typeface = Typeface.MONOSPACE
      textSize = TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_SP,
        TermFontSize.clamp(fontSizeSp.toDouble()).toFloat(),
        resources.displayMetrics,
      )
    }
    paint.measureText("X")
  }

  private fun TerminalRenderer.fontLineSpacingSafe(): Int = getFontLineSpacing().takeIf { it > 0 } ?: run {
    val paint = Paint().apply {
      typeface = Typeface.MONOSPACE
      textSize = TypedValue.applyDimension(
        TypedValue.COMPLEX_UNIT_SP,
        TermFontSize.clamp(fontSizeSp.toDouble()).toFloat(),
        resources.displayMetrics,
      )
    }
    kotlin.math.ceil(paint.fontSpacing.toDouble()).toInt().coerceAtLeast(1)
  }

  private fun applyTheme() {
    val current = terminal ?: run {
      setBackgroundColor(TermTheme.named(themeName).background.argb())
      return
    }
    val colors = TermTheme.named(themeName)
    colors.ansi.forEachIndexed { index, rgb ->
      if (index < 16) current.mColors.mCurrentColors[index] = rgb.argb()
    }
    current.mColors.mCurrentColors[TextStyle.COLOR_INDEX_FOREGROUND] = colors.foreground.argb()
    current.mColors.mCurrentColors[TextStyle.COLOR_INDEX_BACKGROUND] = colors.background.argb()
    current.mColors.mCurrentColors[TextStyle.COLOR_INDEX_CURSOR] = colors.cursor.argb()
    setBackgroundColor(colors.background.argb())
  }

  private fun resetTerminal() {
    terminal?.reset()
    terminal?.screen?.clearTranscript()
    topRowValue = 0
    applyTheme()
    invalidate()
  }

  private fun syncInputDestination() {
    val id = attachedId
    inputSender.setDestination(id, id == null && sshIdValue.isEmpty() && debugEchoValue)
  }

  private fun activeTranscriptRows(): Int =
    terminal?.takeUnless { it.isAlternateBufferActive }?.screen?.activeTranscriptRows ?: 0

  private fun clampTopRow(value: Int) {
    val max = activeTranscriptRows()
    topRowValue = if (terminal?.isAlternateBufferActive == true) 0 else value.coerceIn(-max, 0)
    invalidate()
  }

  private fun finishTouch() {
    velocityTracker?.recycle()
    velocityTracker = null
    dragging = false
    parent?.requestDisallowInterceptTouchEvent(false)
  }

  private fun emitCommandUnavailable(name: String) {
    ai.gxlself.remotescreen.rsnative.common.RsLog.marker(
      "TERMVIEW",
      mapOf("command" to name, "impl" to false),
    )
  }
}
