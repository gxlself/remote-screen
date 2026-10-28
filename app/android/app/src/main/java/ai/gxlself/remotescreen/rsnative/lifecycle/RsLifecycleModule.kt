package ai.gxlself.remotescreen.rsnative.lifecycle

import ai.gxlself.remotescreen.logic.lifecycle.PowerState
import ai.gxlself.remotescreen.specs.NativeRsLifecycleSpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.WritableMap

class RsLifecycleModule(reactContext: ReactApplicationContext) : NativeRsLifecycleSpec(reactContext), LifecycleMonitor.Emitter {
  init {
    LifecycleBootstrap.start(reactContext.applicationContext as android.app.Application)
    LifecycleMonitor.registerEmitter(this)
  }

  override fun getName(): String = NAME

  override fun getTypedExportedConstants(): Map<String, Any> =
    mapOf("previousRunEndedInBackground" to LifecycleMonitor.previousRunEndedInBackground())

  override fun setIdleTimerDisabled(disabled: Boolean) {
    LifecycleMonitor.setIdleTimerDisabled(disabled)
  }

  override fun setBackgroundKeepAlive(enabled: Boolean) {
    LifecycleMonitor.setBackgroundKeepAlive(enabled)
  }

  override fun getPowerState(promise: Promise) {
    promise.resolve(powerStateMap(LifecycleMonitor.currentPowerState()))
  }

  override fun onPowerChange(state: PowerState) {
    emitOnPowerChange(powerStateMap(state))
  }

  override fun onBackgroundTask(event: BackgroundTaskEvent) {
    emitOnBackgroundTask(
      Arguments.createMap().apply {
        putString("event", event.event)
        putString("reason", event.reason)
        putDouble("heldMs", event.heldMs)
      },
    )
  }

  override fun invalidate() {
    LifecycleMonitor.unregisterEmitter(this)
    super.invalidate()
  }

  private fun powerStateMap(state: PowerState): WritableMap = Arguments.createMap().apply {
    putBoolean("lowPower", state.lowPower)
    putBoolean("lowPowerMode", state.lowPowerMode)
    putDouble("batteryLevel", state.batteryLevel)
    putBoolean("charging", state.charging)
  }

  companion object {
    const val NAME = "RsLifecycle"
  }
}
