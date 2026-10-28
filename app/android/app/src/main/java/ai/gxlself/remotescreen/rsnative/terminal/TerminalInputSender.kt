package ai.gxlself.remotescreen.rsnative.terminal

import ai.gxlself.remotescreen.BuildConfig
import ai.gxlself.remotescreen.logic.terminal.TermKeySequences
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsTermNative
import android.os.Handler
import android.os.Looper
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.ThreadFactory

/** Serializes terminal input and protects queued bytes with a destination generation. */
class TerminalInputSender(private val view: RsTerminalView) {
  private val lock = Any()
  private val mainHandler = Handler(Looper.getMainLooper())
  private val executor: ExecutorService = Executors.newSingleThreadExecutor(
    ThreadFactory { runnable ->
      Thread(runnable, "rs-term-input").apply { isDaemon = true }
    },
  )
  private var sshId: Long? = null
  private var debugEcho = false
  private var generation = 0L

  fun setDestination(sshId: Long?, debugEcho: Boolean) {
    synchronized(lock) {
      if (this.sshId == sshId && this.debugEcho == debugEcho) return
      generation++
      this.sshId = sshId
      this.debugEcho = debugEcho
    }
  }

  fun invalidate() {
    synchronized(lock) {
      generation++
      sshId = null
      debugEcho = false
    }
  }

  fun sendSequence(text: String) {
    sendRaw(text.toByteArray(Charsets.UTF_8))
  }

  fun sendKey(name: String) {
    val appCursor = view.emulator?.isCursorKeysApplicationMode == true
    val mods = view.modifiers
    val bytes = TermKeySequences.bytes(name, appCursor, mods)
    if (bytes.isEmpty()) return
    sendRaw(bytes)
    if (mods != 0) view.emitModifiersConsumed(mods)
  }

  fun sendRaw(bytes: ByteArray) {
    if (bytes.isEmpty()) return
    val owned = bytes.copyOf()
    val (expectedGeneration, destination, echo) = synchronized(lock) {
      Triple(generation, sshId, debugEcho)
    }
    executor.execute {
      synchronized(lock) {
        if (expectedGeneration != generation) return@synchronized
        if (destination != null) {
          // Keep invalidation serialized with the JNI call so queued input cannot
          // cross an attachment generation boundary.
          RsTermNative.input(destination, owned)
          return@synchronized
        }
      }
      if (BuildConfig.DEBUG && echo) {
        RsLog.marker("TERMVIEW-INPUT", mapOf("hex" to hex(owned)))
        val echoed = ByteArray(owned.size + owned.count { it == CARRIAGE_RETURN })
        var outputOffset = 0
        owned.forEach { byte ->
          echoed[outputOffset++] = byte
          if (byte == CARRIAGE_RETURN) echoed[outputOffset++] = LINE_FEED
        }
        mainHandler.post {
          val current = synchronized(lock) { expectedGeneration == generation }
          if (current) view.feedLocal(echoed)
        }
      }
    }
  }

  private fun hex(bytes: ByteArray): String =
    bytes.joinToString(separator = "") { byte -> "%02x".format(byte.toInt() and 0xff) }

  private companion object {
    const val CARRIAGE_RETURN: Byte = 0x0d
    const val LINE_FEED: Byte = 0x0a
  }
}
