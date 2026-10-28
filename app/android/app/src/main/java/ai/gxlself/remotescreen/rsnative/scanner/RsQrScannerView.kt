package ai.gxlself.remotescreen.rsnative.scanner

import android.Manifest
import android.app.admin.DevicePolicyManager
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.os.SystemClock
import android.util.Log
import android.widget.FrameLayout
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReactContext
import com.facebook.react.modules.core.PermissionAwareActivity
import com.facebook.react.modules.core.PermissionListener
import ai.gxlself.remotescreen.logic.scanner.LumaPlane
import ai.gxlself.remotescreen.logic.scanner.QrScanThrottle
import ai.gxlself.remotescreen.logic.scanner.ScannerPermission
import ai.gxlself.remotescreen.logic.scanner.ScannerPermissionDecision
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents
import com.google.zxing.BinaryBitmap
import com.google.zxing.ChecksumException
import com.google.zxing.FormatException
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.BarcodeFormat
import com.google.zxing.DecodeHintType
import java.util.EnumMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** Fabric view that owns a CameraX preview and QR analysis pipeline. */
class RsQrScannerView(context: Context) : FrameLayout(context), LifecycleOwner,
  PermissionListener {
  private val reactContext = context as? ReactContext
  private val registry = LifecycleRegistry(this)
  private val previewView = PreviewView(context)
  private val analysisExecutor: ExecutorService = Executors.newSingleThreadExecutor()
  private val throttle = QrScanThrottle()
  private var cameraProvider: ProcessCameraProvider? = null
  private var camera: Camera? = null
  private var imageAnalysis: ImageAnalysis? = null
  private var activeValue = false
  private var torchValue = false
  private var attached = false
  private var foreground = false
  private var destroyed = false
  private var requestingPermission = false
  private var permissionDenied = false
  private var lastState: String? = null
  private var lastHasTorch = false

  private val hostLifecycleListener = object : com.facebook.react.bridge.LifecycleEventListener {
    override fun onHostResume() {
      foreground = true
      refreshLifecycle()
      reconcile()
    }

    override fun onHostPause() {
      foreground = false
      refreshLifecycle()
      reconcile()
    }

    override fun onHostDestroy() {
      foreground = false
      refreshLifecycle()
      reconcile()
    }
  }

  override val lifecycle: Lifecycle
    get() = registry

  var active: Boolean
    get() = activeValue
    set(value) {
      if (activeValue == value) return
      activeValue = value
      if (!value) torchValue = false
      refreshLifecycle()
      reconcile()
    }

  var torch: Boolean
    get() = torchValue
    set(value) {
      torchValue = value
      applyTorch()
    }

  init {
    previewView.implementationMode = PreviewView.ImplementationMode.COMPATIBLE
    addView(previewView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    registry.currentState = Lifecycle.State.CREATED
    reactContext?.addLifecycleEventListener(hostLifecycleListener)
  }

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    attached = true
    refreshLifecycle()
    reconcile()
  }

  override fun onDetachedFromWindow() {
    attached = false
    torchValue = false
    refreshLifecycle()
    stopCamera()
    super.onDetachedFromWindow()
  }

  fun dispose() {
    if (destroyed) return
    destroyed = true
    activeValue = false
    torchValue = false
    reactContext?.removeLifecycleEventListener(hostLifecycleListener)
    stopCamera()
    registry.currentState = Lifecycle.State.DESTROYED
    analysisExecutor.shutdownNow()
  }

  private fun refreshLifecycle() {
    if (destroyed) return
    registry.currentState = if (activeValue && attached && foreground) {
      Lifecycle.State.RESUMED
    } else {
      Lifecycle.State.CREATED
    }
  }

  private fun reconcile() {
    if (destroyed || !activeValue || !attached || !foreground) {
      stopCamera()
      return
    }
    if (requestingPermission || lastState == "running" || lastState == "starting") return
    emitState("starting")
    val hasDevice = context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)
    val policyDisabled = try {
      (context.getSystemService(Context.DEVICE_POLICY_SERVICE) as? DevicePolicyManager)
        ?.getCameraDisabled(null) == true
    } catch (_: SecurityException) {
      false
    }
    if (!hasDevice) {
      emitState("unavailable")
      return
    }
    val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
      PackageManager.PERMISSION_GRANTED
    if (!granted && permissionDenied && !policyDisabled) {
      emitState("denied")
      return
    }
    when (ScannerPermission.decide(ScannerPermission.fromAndroid(granted, policyDisabled), hasDevice)) {
      ScannerPermissionDecision.UNAVAILABLE -> emitState("unavailable")
      ScannerPermissionDecision.RESTRICTED -> emitState("restricted")
      ScannerPermissionDecision.DENIED -> emitState("denied")
      ScannerPermissionDecision.REQUEST -> requestCameraPermission()
      ScannerPermissionDecision.START -> bindCamera()
    }
  }

  private fun requestCameraPermission() {
    val activity = reactContext?.currentActivity
    if (activity !is PermissionAwareActivity) {
      emitState("error", "Camera permission activity unavailable")
      return
    }
    requestingPermission = true
    activity.requestPermissions(arrayOf(Manifest.permission.CAMERA), REQUEST_CODE, this)
  }

  override fun onRequestPermissionsResult(
    requestCode: Int,
    permissions: Array<String>,
    grantResults: IntArray,
  ): Boolean {
    if (requestCode != REQUEST_CODE) return false
    requestingPermission = false
    if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
      permissionDenied = false
      lastState = null
      reconcile()
    } else {
      permissionDenied = true
      emitState("denied")
    }
    return true
  }

  private fun bindCamera() {
    val future = try {
      ProcessCameraProvider.getInstance(context)
    } catch (error: Throwable) {
      emitState("error", error.message.orEmpty())
      return
    }
    future.addListener({
      if (destroyed || !activeValue || !attached || !foreground) return@addListener
      try {
        val provider = future.get()
        cameraProvider = provider
        provider.unbindAll()
        val preview = Preview.Builder().build().also { it.setSurfaceProvider(previewView.surfaceProvider) }
        val selector = ResolutionSelector.Builder()
          .setResolutionStrategy(
            ResolutionStrategy(
              android.util.Size(1280, 720),
              ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER,
            ),
          ).build()
        val analysis = ImageAnalysis.Builder()
          .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
          .setResolutionSelector(selector)
          .build()
        analysis.setAnalyzer(analysisExecutor, ::analyze)
        imageAnalysis = analysis
        camera = provider.bindToLifecycle(
          this,
          CameraSelector.DEFAULT_BACK_CAMERA,
          preview,
          analysis,
        )
        emitState("running", hasTorch = camera?.cameraInfo?.hasFlashUnit() == true)
        applyTorch()
      } catch (error: Throwable) {
        stopCamera()
        emitState("error", error.message.orEmpty())
      }
    }, ContextCompat.getMainExecutor(context))
  }

  private fun stopCamera() {
    camera?.cameraControl?.enableTorch(false)
    camera = null
    imageAnalysis?.clearAnalyzer()
    imageAnalysis = null
    cameraProvider?.unbindAll()
    cameraProvider = null
    lastState = null
    lastHasTorch = false
  }

  private fun applyTorch() {
    val currentCamera = camera ?: return
    if (lastState == "running" && currentCamera.cameraInfo.hasFlashUnit()) {
      currentCamera.cameraControl.enableTorch(torchValue && activeValue && attached && foreground)
    } else {
      currentCamera.cameraControl.enableTorch(false)
    }
  }

  private fun analyze(image: ImageProxy) {
    try {
      val plane = image.planes.firstOrNull() ?: return
      if (image.format != ImageFormat.YUV_420_888) return
      val buffer = plane.buffer.duplicate()
      val bytes = ByteArray(buffer.remaining())
      buffer.get(bytes)
      val width = image.width
      val height = image.height
      val luma = LumaPlane.pack(bytes, plane.rowStride, plane.pixelStride, width, height)
      val source = PlanarYUVLuminanceSource(luma, width, height, 0, 0, width, height, false)
      val bitmap = BinaryBitmap(HybridBinarizer(source))
      val hints = EnumMap<DecodeHintType, Any>(DecodeHintType::class.java).apply {
        put(DecodeHintType.POSSIBLE_FORMATS, listOf(BarcodeFormat.QR_CODE))
      }
      val result = try {
        QRCodeReader().decode(bitmap, hints)
      } catch (_: NotFoundException) {
        null
      } catch (_: ChecksumException) {
        null
      } catch (_: FormatException) {
        null
      }
      val text = result?.text?.takeIf { it.isNotEmpty() } ?: return
      if (!throttle.accept(text, SystemClock.elapsedRealtime())) return
      postCode(text)
    } catch (_: Throwable) {
      // Individual malformed frames must not stop the analyzer.
    } finally {
      image.close()
    }
  }

  private fun postCode(text: String) {
    post {
      if (!destroyed && activeValue && attached && foreground) {
        val data = Arguments.createMap().apply { putString("text", text) }
        RsViewEvents.emit(this, "topCode", data)
        Log.i("RS", "RS-SCANNER {\"code\":{\"len\":${text.length}}}")
      }
    }
  }

  private fun emitState(state: String, message: String = "", hasTorch: Boolean = false) {
    if (state != "running") {
      torchValue = false
      camera?.cameraControl?.enableTorch(false)
    }
    if (state == lastState) return
    lastState = state
    lastHasTorch = hasTorch
    val safeMessage = message.take(200)
    post {
      if (destroyed || !activeValue || !attached || !foreground) return@post
      val data = Arguments.createMap().apply {
        putString("state", state)
        putString("message", safeMessage)
        putBoolean("hasTorch", hasTorch)
      }
      RsViewEvents.emit(this, "topScannerState", data)
      RsLog.marker("SCANNER", mapOf("state" to state))
    }
  }

  companion object {
    private const val REQUEST_CODE = 4301
  }
}
