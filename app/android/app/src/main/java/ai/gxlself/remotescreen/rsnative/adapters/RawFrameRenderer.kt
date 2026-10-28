package ai.gxlself.remotescreen.rsnative.adapters

import android.graphics.Bitmap
import android.graphics.Paint
import android.graphics.Rect
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.view.Surface
import androidx.core.graphics.createBitmap
import ai.gxlself.remotescreen.logic.adapters.RawPacing
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.jni.RsRawNative
import java.nio.ByteBuffer

/**
 * Pulls an adapter session's framebuffer on its own rs-raw thread and draws it with
 * Surface.lockHardwareCanvas. Not ANativeWindow_lock: from Android 12 a buffer whose size
 * differs from the SurfaceView is dropped under the FREEZE scaling mode, which the NDK
 * cannot change; the hardware canvas always has the window's size and HWUI scales.
 */
class RawFrameRenderer(private val sessionId: Long, private val listener: Listener) {
  interface Listener {
    /** Called on rs-raw after a frame reached the Surface; copyUs is 0 for a redraw. */
    fun onRawPresented(width: Int, height: Int, copyUs: Long)
  }

  private val thread = HandlerThread("rs-raw").apply { start() }
  private val handler = Handler(thread.looper)
  private val pullTask = Runnable { pull() }

  // Guarded by lock: onReady arrives on a Rust worker.
  private val lock = Any()
  private val pacing = RawPacing()
  private var released = false
  private var readySeq = 0L
  private var readyWidth = 0
  private var readyHeight = 0

  // rs-raw state.
  private var surface: Surface? = null
  private var buffer: ByteBuffer? = null
  private var bitmap: Bitmap? = null
  private var lastSeq = 0L
  private val rect = IntArray(4)
  private val bounds = Rect()
  private val paint = Paint(Paint.FILTER_BITMAP_FLAG)

  /** Any thread. Redraws the current picture at once, without waiting for a remote change. */
  fun setSurface(surface: Surface?) {
    synchronized(lock) {
      if (released) return
      handler.post {
        this.surface = surface
        val image = bitmap
        if (surface != null && image != null && lastSeq != 0L) draw(image, copyUs = 0)
      }
    }
  }

  /** Rust thread: only records the seq and schedules a pull. */
  fun onReady(seq: Long, width: Int, height: Int) {
    synchronized(lock) {
      if (released) return
      if (java.lang.Long.compareUnsigned(seq, readySeq) >= 0) {
        readySeq = seq
        readyWidth = width
        readyHeight = height
      }
      val delay = pacing.ready(seq, SystemClock.uptimeMillis()) ?: return
      handler.postDelayed(pullTask, delay)
    }
  }

  /** Main thread. Drops queued work; the Bitmap is recycled on rs-raw after any running pull. */
  fun release() {
    synchronized(lock) {
      if (released) return
      released = true
      pacing.reset()
      handler.removeCallbacksAndMessages(null)
      handler.post {
        surface = null
        bitmap?.recycle()
        bitmap = null
        buffer = null
      }
    }
    thread.quitSafely()
  }

  private fun pull() {
    val target: Long
    val width: Int
    val height: Int
    synchronized(lock) {
      if (released) return
      target = pacing.begin(SystemClock.uptimeMillis())
      width = readyWidth
      height = readyHeight
    }
    val pulled = copyAndDraw(target, width, height)
    synchronized(lock) {
      if (released) return
      val nowMs = SystemClock.uptimeMillis()
      // A resize announced during the copy makes it fail (0); pull again at the new size.
      val delay = pacing.end(pulled, nowMs)
        ?: if (pulled == 0L && (readyWidth != width || readyHeight != height)) {
          pacing.ready(readySeq, nowMs)
        } else {
          null
        }
      if (delay != null) handler.postDelayed(pullTask, delay)
    }
  }

  /** Returns the seq for RawPacing.end: the copied seq, or 0 when nothing was copied. */
  private fun copyAndDraw(target: Long, width: Int, height: Int): Long {
    if (width <= 0 || height <= 0) return 0
    val current = bitmap
    val image = if (current != null && current.width == width && current.height == height) {
      // Nothing newer than the bitmap has been announced.
      if (lastSeq != 0L && java.lang.Long.compareUnsigned(target, lastSeq) <= 0) return lastSeq
      current
    } else {
      allocate(width, height) ?: return 0
    }
    val dst = buffer ?: return 0
    val startedNs = SystemClock.elapsedRealtimeNanos()
    val seq = RsRawNative.copy(sessionId, lastSeq, dst, width * 4, width, height, rect)
    if (seq != 0L && rect[2] > 0 && rect[3] > 0) {
      dst.rewind()
      image.copyPixelsFromBuffer(dst)
      lastSeq = seq
    }
    val copyUs = (SystemClock.elapsedRealtimeNanos() - startedNs) / 1_000
    if (lastSeq != 0L) draw(image, copyUs)
    return seq
  }

  private fun allocate(width: Int, height: Int): Bitmap? {
    bitmap?.recycle()
    bitmap = null
    buffer = null
    lastSeq = 0
    val bytes = width.toLong() * height * 4
    val allocated = if (bytes > Int.MAX_VALUE) null else try {
      ByteBuffer.allocateDirect(bytes.toInt()) to
        createBitmap(width, height, Bitmap.Config.ARGB_8888)
    } catch (_: OutOfMemoryError) {
      null
    }
    if (allocated == null) {
      RsLog.marker("SESSVIEW", mapOf("error" to "raw-alloc", "width" to width, "height" to height))
      return null
    }
    buffer = allocated.first
    return allocated.second.also {
      it.setHasAlpha(false)
      bitmap = it
    }
  }

  private fun draw(image: Bitmap, copyUs: Long) {
    val target = surface?.takeIf { it.isValid } ?: return
    try {
      val canvas = target.lockHardwareCanvas()
      try {
        bounds.set(0, 0, canvas.width, canvas.height)
        canvas.drawBitmap(image, null, bounds, paint)
      } finally {
        target.unlockCanvasAndPost(canvas)
      }
    } catch (error: IllegalArgumentException) {
      surfaceLost(error)
      return
    } catch (error: IllegalStateException) {
      surfaceLost(error)
      return
    }
    listener.onRawPresented(image.width, image.height, copyUs)
  }

  private fun surfaceLost(error: RuntimeException) {
    RsLog.marker("SESSVIEW", mapOf("raw" to "surface-lost", "type" to error.javaClass.simpleName))
  }
}
