package ai.gxlself.remotescreen.rsnative.lifecycle

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class RsLifecyclePackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsLifecycleModule.NAME) RsLifecycleModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsLifecycleModule.NAME to ReactModuleInfo(
        RsLifecycleModule.NAME,
        RsLifecycleModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
