package ai.gxlself.remotescreen.rsnative.files

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import androidx.core.content.ContextCompat
import androidx.core.net.toUri
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import ai.gxlself.remotescreen.logic.files.FileNameRules
import ai.gxlself.remotescreen.logic.files.InboxCopy
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.specs.NativeRsFilesSpec
import com.facebook.react.bridge.ActivityEventListener
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.UiThreadUtil
import com.facebook.react.bridge.WritableMap
import com.facebook.react.modules.core.DeviceEventManagerModule
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean

class RsFilesModule(
  reactContext: ReactApplicationContext,
) : NativeRsFilesSpec(reactContext), ActivityEventListener {
  private val appContext = reactContext.applicationContext
  private val resolver = reactContext.contentResolver
  private val copyExecutor: ExecutorService = Executors.newSingleThreadExecutor { runnable ->
    Thread(runnable, "rs-files-copy").apply { isDaemon = true }
  }
  private val pickerLock = Any()
  private var pendingPicker: PendingPicker? = null
  private val invalidated = AtomicBoolean(false)
  private var transfersActive = false
  private var transferStartRequested = false
  private val lifecycleObserver = object : DefaultLifecycleObserver {
    override fun onStart(owner: LifecycleOwner) {
      startTransfersIfPossible()
    }
  }

  init {
    reactContext.addActivityEventListener(this)
    UiThreadUtil.runOnUiThread {
      if (!invalidated.get()) {
        ProcessLifecycleOwner.get().lifecycle.addObserver(lifecycleObserver)
      }
    }
  }

  override fun getName(): String = NAME

  override fun getTypedExportedConstants(): Map<String, Any> {
    val directories = directories()
    try {
      ensureDirectories(directories)
    } catch (_: Exception) {
      RsLog.marker("FILES", mapOf("directories" to "error"))
    }
    return mapOf(
      "filesDir" to directories.files.absolutePath,
      "inboxDir" to directories.inbox.absolutePath,
    )
  }

  override fun pickFiles(promise: Promise) {
    val operation = PendingPicker.Pick(promise)
    if (!reservePicker(operation)) return
    try {
      ensureDirectories(directories())
    } catch (error: IOException) {
      clearPicker(operation)
      promise.reject("directories", "无法准备文件目录", error)
      return
    }
    launchPicker(operation, Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "*/*"
      putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    })
  }

  override fun saveToFiles(uri: String, name: String, promise: Promise) {
    val source = uri.toUri().normalizeScheme()
    val operation = PendingPicker.Save(promise, source)
    if (!reservePicker(operation)) return
    if (source.scheme !in setOf(SCHEME_CONTENT, SCHEME_FILE) ||
      (source.scheme == SCHEME_FILE && source.path?.let { File(it).isFile } != true)
    ) {
      clearPicker(operation)
      promise.reject("not_found", "文件不存在")
      return
    }
    launchPicker(operation, Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "*/*"
      putExtra(Intent.EXTRA_TITLE, FileNameRules.sanitize(name))
      addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
    })
  }

  private fun launchPicker(operation: PendingPicker, intent: Intent) {
    UiThreadUtil.runOnUiThread {
      if (invalidated.get()) return@runOnUiThread
      val activity = reactApplicationContext.currentActivity
      if (activity == null) {
        clearPicker(operation)
        operation.promise.reject("no_presenter", "无法打开文件选择器")
        return@runOnUiThread
      }
      try {
        activity.startActivityForResult(intent, operation.requestCode)
      } catch (error: RuntimeException) {
        clearPicker(operation)
        operation.promise.reject("no_presenter", "无法打开文件选择器", error)
      }
    }
  }

  override fun importIncoming(url: String, promise: Promise) {
    val uri = try {
      url.toUri().normalizeScheme()
    } catch (_: IllegalArgumentException) {
      promise.resolve(null)
      return
    }
    val scheme = uri.scheme
    if (scheme != SCHEME_CONTENT && scheme != SCHEME_FILE) {
      promise.resolve(null)
      return
    }

    runCopy(promise, "import_failed") {
      fileMap(copyToInbox(uri))
    }
  }

  override fun setTransfersActive(active: Boolean) {
    UiThreadUtil.runOnUiThread {
      if (invalidated.get()) return@runOnUiThread
      transfersActive = active
      if (active) {
        startTransfersIfPossible()
      } else {
        transferStartRequested = false
        appContext.stopService(RsTransferService.stopIntent(appContext))
      }
    }
  }

  private fun startTransfersIfPossible() {
    if (invalidated.get() || !transfersActive || transferStartRequested) return
    val foreground = ProcessLifecycleOwner.get().lifecycle.currentState
      .isAtLeast(Lifecycle.State.STARTED)
    if (!foreground) return
    try {
      ContextCompat.startForegroundService(appContext, RsTransferService.startIntent(appContext))
      transferStartRequested = true
    } catch (_: RuntimeException) {
      RsLog.marker("FILES", mapOf("transferService" to "start_error"))
    }
  }

  override fun onActivityResult(activity: Activity, requestCode: Int, resultCode: Int, data: Intent?) {
    val operation = takePicker(requestCode) ?: return
    if (resultCode != Activity.RESULT_OK) {
      clearPicker(operation)
      when (operation) {
        is PendingPicker.Pick -> operation.promise.resolve(Arguments.createArray())
        is PendingPicker.Save -> operation.promise.resolve(false)
      }
      return
    }

    when (operation) {
      is PendingPicker.Pick -> finishPick(operation, data)
      is PendingPicker.Save -> finishSave(operation, data)
    }
  }

  override fun onNewIntent(intent: Intent) {
    if (!RsFilesIntents.isShareAction(intent)) {
      return
    }
    RsFilesIntents.streamUris(intent).forEach { uri ->
      try {
        reactApplicationContext
          .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
          .emit("url", Arguments.createMap().apply { putString("url", uri.toString()) })
      } catch (_: RuntimeException) {
        // The JS bridge may be between teardown and recreation during a hot start.
      }
    }
  }

  override fun invalidate() {
    invalidated.set(true)
    reactApplicationContext.removeActivityEventListener(this)
    synchronized(pickerLock) {
      pendingPicker?.promise?.reject("no_presenter", "文件选择器已关闭")
      pendingPicker = null
    }
    copyExecutor.shutdown()
    UiThreadUtil.runOnUiThread {
      ProcessLifecycleOwner.get().lifecycle.removeObserver(lifecycleObserver)
      transfersActive = false
      transferStartRequested = false
      appContext.stopService(RsTransferService.stopIntent(appContext))
    }
    super.invalidate()
  }

  private fun finishPick(operation: PendingPicker.Pick, data: Intent?) {
    val promise = operation.promise
    val uris = dataUris(data)
    if (uris.isEmpty()) {
      clearPicker(operation)
      promise.resolve(Arguments.createArray())
      return
    }
    runCopy(promise, "copy_failed", operation) {
      val copied = mutableListOf<CopiedFile>()
      try {
        val result = Arguments.createArray()
        uris.forEach { uri ->
          val file = copyToInbox(uri)
          copied.add(file)
          result.pushMap(fileMap(file))
        }
        result
      } catch (error: Throwable) {
        copied.forEach { it.file.delete() }
        throw error
      }
    }
  }

  private fun finishSave(operation: PendingPicker.Save, data: Intent?) {
    val destination = data?.data
    if (destination == null) {
      clearPicker(operation)
      operation.promise.resolve(false)
      return
    }
    runCopy(operation.promise, "copy_failed", operation) {
      try {
        copyToDestination(operation.source, destination)
        true
      } catch (error: Throwable) {
        deleteDestination(destination)
        throw error
      }
    }
  }

  private fun dataUris(data: Intent?): List<Uri> {
    if (data == null) {
      return emptyList()
    }
    val result = ArrayList<Uri>()
    val clip = data.clipData
    if (clip != null) {
      for (index in 0 until clip.itemCount) {
        clip.getItemAt(index).uri?.let(result::add)
      }
    }
    data.data?.let { uri ->
      if (!result.contains(uri)) {
        result.add(uri)
      }
    }
    return result
  }

  private fun copyToInbox(uri: Uri): CopiedFile {
    val dirs = directories()
    ensureDirectories(dirs)
    val source = sourceInfo(uri)
    val destination = InboxCopy.copy(dirs.inbox, source.name) {
      openInput(uri) ?: throw IOException("无法打开来源文件")
    }
    return CopiedFile(destination, source.size)
  }

  private fun copyToDestination(source: Uri, destination: Uri) {
    (openInput(source) ?: throw IOException("文件不存在")).use { sourceStream ->
      (openOutput(destination) ?: throw IOException("无法打开保存位置")).use { targetStream ->
        sourceStream.copyTo(targetStream, COPY_BUFFER_SIZE)
      }
    }
  }

  private fun sourceInfo(uri: Uri): SourceInfo {
    return when (uri.scheme?.lowercase()) {
      SCHEME_CONTENT -> contentSourceInfo(uri)
      SCHEME_FILE -> {
        val path = uri.path ?: throw FileNotFoundException("文件不存在")
        val file = File(path)
        if (!file.isFile) {
          throw FileNotFoundException("文件不存在")
        }
        SourceInfo(file.name.ifBlank { FileNameRules.DEFAULT_NAME }, file.length())
      }
      else -> throw FileNotFoundException("不支持的文件地址")
    }
  }

  private fun contentSourceInfo(uri: Uri): SourceInfo {
    var name: String? = null
    var size = -1L
    try {
      resolver.query(
        uri,
        arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
        null,
        null,
        null,
      )
        ?.use { cursor ->
          if (cursor.moveToFirst()) {
            val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (nameIndex >= 0 && !cursor.isNull(nameIndex)) {
              name = cursor.getString(nameIndex)
            }
            val sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE)
            if (sizeIndex >= 0 && !cursor.isNull(sizeIndex)) {
              size = cursor.getLong(sizeIndex).takeIf { it >= 0 } ?: -1L
            }
          }
        }
    } catch (_: RuntimeException) {
      // Metadata is optional; providers can still expose a readable stream without it.
    }
    val fallback = uri.lastPathSegment?.substringAfterLast('/').orEmpty()
    return SourceInfo(name ?: fallback.ifBlank { FileNameRules.DEFAULT_NAME }, size)
  }

  private fun openInput(uri: Uri): InputStream? {
    return when (uri.scheme?.lowercase()) {
      SCHEME_CONTENT -> resolver.openInputStream(uri)
      SCHEME_FILE -> uri.path?.let(::File)?.let(::FileInputStream)
      else -> null
    }
  }

  private fun openOutput(uri: Uri): OutputStream? {
    return when (uri.scheme?.lowercase()) {
      SCHEME_CONTENT -> resolver.openOutputStream(uri)
      SCHEME_FILE -> uri.path?.let(::File)?.let(::FileOutputStream)
      else -> null
    }
  }

  private fun deleteDestination(uri: Uri) {
    try {
      when (uri.scheme?.lowercase()) {
        SCHEME_CONTENT -> {
          if (DocumentsContract.isDocumentUri(appContext, uri)) {
            DocumentsContract.deleteDocument(resolver, uri)
          } else {
            resolver.delete(uri, null, null)
          }
        }
        SCHEME_FILE -> uri.path?.let(::File)?.delete()
      }
    } catch (_: Exception) {
      // A provider may refuse deleting a partially written user destination.
    }
  }

  private fun fileMap(copy: CopiedFile): WritableMap = Arguments.createMap().apply {
    putString("uri", Uri.fromFile(copy.file).toString())
    putString("name", copy.file.name)
    putDouble("size", copy.size.toDouble())
  }

  private fun directories(): Directories = Directories(
    files = File(appContext.filesDir, "Received"),
    inbox = File(appContext.cacheDir, "RsInbox"),
  )

  private fun ensureDirectories(dirs: Directories) {
    listOf(dirs.files, dirs.inbox).forEach { directory ->
      if (!directory.isDirectory && !directory.mkdirs() && !directory.isDirectory) {
        throw IOException("无法准备文件目录")
      }
    }
  }

  private fun reservePicker(operation: PendingPicker): Boolean {
    synchronized(pickerLock) {
      if (invalidated.get()) {
        operation.promise.reject("no_presenter", "文件选择器已关闭")
        return false
      }
      if (pendingPicker != null) {
        operation.promise.reject("E_BUSY", "文件选择器正在显示")
        return false
      }
      pendingPicker = operation
      return true
    }
  }

  private fun takePicker(requestCode: Int): PendingPicker? {
    synchronized(pickerLock) {
      val operation = pendingPicker ?: return null
      if (operation.requestCode != requestCode || operation.resultReceived) {
        return null
      }
      operation.resultReceived = true
      return operation
    }
  }

  private fun clearPicker(operation: PendingPicker) {
    synchronized(pickerLock) {
      if (pendingPicker === operation) {
        pendingPicker = null
      }
    }
  }

  private fun runCopy(
    promise: Promise,
    errorCode: String,
    operation: PendingPicker? = null,
    action: () -> Any?,
  ) {
    try {
      copyExecutor.execute {
        try {
          val result = try {
            action()
          } finally {
            if (operation != null) clearPicker(operation)
          }
          promise.resolve(result)
        } catch (error: Exception) {
          promise.reject(errorCode, error.message ?: "无法复制文件", error)
        }
      }
    } catch (error: RejectedExecutionException) {
      if (operation != null) clearPicker(operation)
      promise.reject(errorCode, "文件服务已关闭", error)
    }
  }

  private data class Directories(val files: File, val inbox: File)

  private data class SourceInfo(val name: String, val size: Long)

  private data class CopiedFile(val file: File, val size: Long)

  private sealed class PendingPicker(val promise: Promise, val requestCode: Int) {
    var resultReceived = false
    class Pick(promise: Promise) : PendingPicker(promise, REQUEST_PICK)
    class Save(promise: Promise, val source: Uri) : PendingPicker(promise, REQUEST_SAVE)
  }

  companion object {
    const val NAME = "RsFiles"
    private const val REQUEST_PICK = 7301
    private const val REQUEST_SAVE = 7302
    private const val COPY_BUFFER_SIZE = 64 * 1024
    private const val SCHEME_CONTENT = "content"
    private const val SCHEME_FILE = "file"
  }
}
