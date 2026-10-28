package ai.gxlself.remotescreen.rsnative.keyboard

import ai.gxlself.remotescreen.logic.keyboard.CommitPlanner
import ai.gxlself.remotescreen.logic.keyboard.HardwareKeyPlanner
import ai.gxlself.remotescreen.logic.keyboard.InputMode
import ai.gxlself.remotescreen.logic.keyboard.KeyAction
import ai.gxlself.remotescreen.logic.keyboard.KeyboardOs
import ai.gxlself.remotescreen.logic.keyboard.Modifiers
import ai.gxlself.remotescreen.logic.keyboard.OsMap
import ai.gxlself.remotescreen.logic.keyboard.SessionShortcut
import ai.gxlself.remotescreen.logic.keyboard.SessionShortcuts
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import android.content.Context
import android.graphics.Color
import android.os.Build
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.facebook.react.bridge.Arguments

/** Invisible one-pixel editor used to capture Android software and hardware keys. */
class RsKeyCaptureView(context: Context) : View(context) {
  private var requestedActive = false
  private var mode = InputMode.TEXT
  private var keyboardOs = KeyboardOs.MACOS
  private var suppliedStickyMods = 0
  private var effectiveStickyMods = 0
  private var currentMods = 0
  private val pressedUsages = LinkedHashSet<Int>()
  private val textRoutedKeyCodes = HashSet<Int>()
  // ⌃⌥⌘ session shortcuts: their up events stay here too.
  private val shortcutKeyCodes = HashSet<Int>()
  private var inputConnection: RsKeyCaptureInputConnection? = null
  private var lastReportedFocus: Boolean? = null

  private val frameObserver = KeyboardFrameObserver(this) { height, visible, hardware ->
    val event = Arguments.createMap()
    event.putDouble("height", height.toDouble())
    event.putBoolean("visible", visible)
    event.putBoolean("hardware", hardware)
    RsViewEvents.emit(this, "topKeyboardFrame", event)
  }

  init {
    isFocusable = true
    isFocusableInTouchMode = true
    alpha = 0f
    setBackgroundColor(Color.TRANSPARENT)
    importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    setOnFocusChangeListener { _, hasFocus ->
      if (!hasFocus) {
        releasePressedKeys()
      }
      if (lastReportedFocus != hasFocus) {
        lastReportedFocus = hasFocus
        val event = Arguments.createMap()
        event.putBoolean("active", hasFocus)
        RsViewEvents.emit(this, "topActiveChange", event)
      }
    }
  }

  override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
    setMeasuredDimension(1, 1)
  }

  override fun onCheckIsTextEditor(): Boolean = true

  override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
    outAttrs.inputType =
      if (mode == InputMode.TEXT) {
        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
      } else {
        InputType.TYPE_CLASS_TEXT or
          InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
          InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
      }
    outAttrs.imeOptions =
      EditorInfo.IME_FLAG_NO_EXTRACT_UI or
        EditorInfo.IME_FLAG_NO_FULLSCREEN or
        EditorInfo.IME_ACTION_NONE
    return RsKeyCaptureInputConnection(this).also { inputConnection = it }
  }

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    frameObserver.start()
    if (requestedActive) {
      post {
        if (requestedActive && isAttachedToWindow) {
          requestFocus()
          showKeyboard()
        }
      }
    }
  }

  override fun onDetachedFromWindow() {
    frameObserver.stop()
    releasePressedKeys()
    inputConnection = null
    super.onDetachedFromWindow()
  }

  fun setActive(next: Boolean) {
    if (requestedActive == next) {
      if (next && isAttachedToWindow && !hasFocus()) requestFocus()
      return
    }
    requestedActive = next
    RsLog.marker("KEYCAP", mapOf("kind" to "active", "active" to next))
    if (next) {
      requestFocus()
      post { if (requestedActive && isAttachedToWindow) showKeyboard() }
    } else {
      hideKeyboard()
      clearFocus()
    }
  }

  fun setInputMode(next: String?) {
    val nextMode = if (next == "keys") InputMode.KEYS else InputMode.TEXT
    if (mode == nextMode) return
    releasePressedKeys()
    mode = nextMode
    RsLog.marker("KEYCAP", mapOf("kind" to "inputMode", "mode" to mode.name.lowercase()))
    val manager = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
    manager?.restartInput(this)
  }

  fun setStickyMods(next: Int) {
    if (suppliedStickyMods == next) return
    suppliedStickyMods = next
    effectiveStickyMods = next
  }

  fun setOs(next: String?) {
    val nextOs = if (next == "windows") KeyboardOs.WINDOWS else KeyboardOs.MACOS
    if (keyboardOs == nextOs) return
    releasePressedKeys()
    keyboardOs = nextOs
  }

  /** Handles a key event delivered by Android's hardware dispatcher or an IME. */
  fun handleKeyEvent(event: KeyEvent): Boolean {
    return when (event.action) {
      KeyEvent.ACTION_DOWN -> handleKeyDown(event.keyCode, event)
      KeyEvent.ACTION_UP -> handleKeyUp(event.keyCode, event)
      KeyEvent.ACTION_MULTIPLE -> handleKeyMultiple(event.keyCode, event.repeatCount, event)
      else -> false
    }
  }

  override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
    handleKeyDown(keyCode, event)

  override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
    handleKeyUp(keyCode, event)

  override fun onKeyMultiple(keyCode: Int, count: Int, event: KeyEvent): Boolean =
    handleKeyMultiple(keyCode, count, event) || super.onKeyMultiple(keyCode, count, event)

  fun handleKeyMultiple(keyCode: Int, count: Int, event: KeyEvent): Boolean {
    val repeats = count.coerceAtLeast(1)
    if (mode == InputMode.TEXT && isPlainPrinting(event)) {
      val characters = if (Build.VERSION.SDK_INT >= 29) event.characters else null
      if (!characters.isNullOrEmpty()) {
        dispatchCommittedText(characters)
        return true
      }
      val unicode = event.unicodeChar
      if (unicode != 0) {
        repeat(repeats) { dispatchCommittedText(String(Character.toChars(unicode))) }
        return true
      }
    }
    var handled = false
    repeat(repeats) {
      val actions = HardwareKeyPlanner.plan(keyCode, down = true, event.metaState, keyboardOs, effectiveStickyMods)
      if (actions != null) {
        handled = true
        applyHardwareActions(actions)
        applyHardwareActions(
          HardwareKeyPlanner.plan(keyCode, down = false, event.metaState, keyboardOs, effectiveStickyMods)
            ?: emptyList(),
        )
      }
    }
    return handled
  }

  fun commitText(text: String) {
    if (text.isEmpty()) return
    dispatchCommittedText(text)
  }

  fun sendEnter() {
    emitKey(0x28, true, 0)
    emitKey(0x28, false, 0)
  }

  fun sendBackspace() {
    emitKey(0x2A, true, 0)
    emitKey(0x2A, false, 0)
  }

  private fun handleKeyDown(keyCode: Int, event: KeyEvent): Boolean {
    val shortcut = SessionShortcuts.forKeyEvent(keyCode, event.metaState)
    if (shortcut != null) {
      shortcutKeyCodes += keyCode
      if (event.repeatCount == 0) emitShortcut(shortcut)
      return true
    }
    if (mode == InputMode.TEXT && isPlainPrinting(event)) {
      val unicode = event.unicodeChar
      if (unicode != 0) {
        dispatchCommittedText(String(Character.toChars(unicode)))
        textRoutedKeyCodes += keyCode
        return true
      }
    }
    return dispatchHardware(keyCode, true, event.metaState) || super.onKeyDown(keyCode, event)
  }

  private fun handleKeyUp(keyCode: Int, event: KeyEvent): Boolean {
    // A printable key routed through the text contract has no matching HID
    // key-up. Consume the physical up event so it cannot leak a duplicate raw
    // key to the host.
    if (textRoutedKeyCodes.remove(keyCode) || shortcutKeyCodes.remove(keyCode)) {
      return true
    }
    return dispatchHardware(keyCode, false, event.metaState) || super.onKeyUp(keyCode, event)
  }

  private fun isPlainPrinting(event: KeyEvent): Boolean {
    val blocked = KeyEvent.META_CTRL_ON or KeyEvent.META_ALT_ON or KeyEvent.META_META_ON
    return event.isPrintingKey && (event.metaState and blocked) == 0
  }

  private fun dispatchHardware(keyCode: Int, down: Boolean, metaState: Int): Boolean {
    val actions = HardwareKeyPlanner.plan(keyCode, down, metaState, keyboardOs, effectiveStickyMods)
      ?: return false
    applyHardwareActions(actions)
    return true
  }

  private fun applyHardwareActions(actions: List<KeyAction>) {
    for (action in actions) {
      when (action) {
        is KeyAction.Key -> {
          currentMods = action.mods
          if (action.down) {
            pressedUsages += action.usage
          } else {
            pressedUsages -= action.usage
          }
          emitKey(action.usage, action.down, action.mods)
        }
        is KeyAction.StickyConsumed -> consumeSticky(action.mods)
        is KeyAction.Text -> emitText(action.text)
      }
    }
  }

  private fun dispatchCommittedText(text: String) {
    val actions = CommitPlanner.plan(text, mode, effectiveStickyMods, IcuGraphemeSplitter())
    for (action in actions) {
      when (action) {
        is KeyAction.Text -> emitText(action.text)
        is KeyAction.Key -> {
          val (usage, mods) = OsMap.remap(action.usage, action.mods, keyboardOs)
          emitKey(usage, action.down, mods)
        }
        is KeyAction.StickyConsumed -> consumeSticky(action.mods)
      }
    }
  }

  private fun consumeSticky(mods: Int) {
    effectiveStickyMods = 0
    val event = Arguments.createMap()
    event.putInt("mods", mods)
    RsViewEvents.emit(this, "topStickyConsumed", event)
  }

  private fun emitKey(usage: Int, down: Boolean, mods: Int) {
    val event = Arguments.createMap()
    event.putInt("usage", usage)
    event.putBoolean("down", down)
    event.putInt("mods", mods)
    RsViewEvents.emit(this, "topKey", event)
  }

  private fun emitShortcut(shortcut: SessionShortcut) {
    val event = Arguments.createMap()
    event.putString("action", shortcut.action)
    event.putInt("index", shortcut.index)
    RsViewEvents.emit(this, "topShortcut", event)
  }

  private fun emitText(text: String) {
    val event = Arguments.createMap()
    event.putString("text", text)
    RsViewEvents.emit(this, "topText", event)
  }

  fun emitComposition(text: String) {
    val event = Arguments.createMap()
    event.putString("text", text)
    RsViewEvents.emit(this, "topComposition", event)
  }

  private fun releasePressedKeys() {
    if (pressedUsages.isEmpty()) {
      textRoutedKeyCodes.clear()
      currentMods = 0
      return
    }
    val usages = pressedUsages.toList().sortedWith(compareBy<Int> { it in 0xE0..0xE7 }.thenBy { it })
    for (usage in usages) {
      val mods = if (usage in 0xE0..0xE7) {
        Modifiers.after(usage, down = false, currentMods)
      } else {
        currentMods
      }
      currentMods = mods
      emitKey(usage, false, mods)
    }
    pressedUsages.clear()
    textRoutedKeyCodes.clear()
    currentMods = 0
  }

  private fun showKeyboard() {
    if (Build.VERSION.SDK_INT >= 30) {
      ViewCompat.getWindowInsetsController(this)?.show(WindowInsetsCompat.Type.ime())
    } else {
      val manager = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
      manager?.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }
  }

  private fun hideKeyboard() {
    if (Build.VERSION.SDK_INT >= 30) {
      ViewCompat.getWindowInsetsController(this)?.hide(WindowInsetsCompat.Type.ime())
    } else {
      val manager = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
      manager?.hideSoftInputFromWindow(windowToken, 0)
    }
  }
}
