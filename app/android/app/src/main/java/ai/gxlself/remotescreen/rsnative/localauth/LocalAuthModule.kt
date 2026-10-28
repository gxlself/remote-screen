package ai.gxlself.remotescreen.rsnative.localauth

import ai.gxlself.remotescreen.specs.NativeLocalAuthSpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.LifecycleEventListener
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.WritableMap
import com.facebook.react.module.annotations.ReactModule

@ReactModule(name = LocalAuthModule.NAME)
class LocalAuthModule(reactContext: ReactApplicationContext) :
  NativeLocalAuthSpec(reactContext), LifecycleEventListener {
  private val service = LocalAuthService.get(reactContext)

  override fun getName(): String = NAME

  override fun initialize() {
    super.initialize()
    reactApplicationContext.addLifecycleEventListener(this)
  }

  override fun invalidate() {
    reactApplicationContext.removeLifecycleEventListener(this)
    super.invalidate()
  }

  override fun authenticate(reason: String, fallbackTitle: String, promise: Promise) {
    service.authenticate(reason, fallbackTitle) { result -> promise.resolve(result) }
  }

  override fun getBiometryType(): String = service.getBiometryType()

  override fun canAuthenticate(): WritableMap {
    val capabilities = service.canAuthenticate()
    return Arguments.createMap().apply {
      putBoolean("biometrics", capabilities.biometrics)
      putBoolean("passcode", capabilities.passcode)
    }
  }

  override fun setPrivacyCover(enabled: Boolean) {
    service.setPrivacyCover(enabled)
  }

  override fun onHostResume() {
    service.onHostResume()
  }

  override fun onHostPause() {}
  override fun onHostDestroy() {}

  companion object {
    const val NAME = "LocalAuth"
  }
}
