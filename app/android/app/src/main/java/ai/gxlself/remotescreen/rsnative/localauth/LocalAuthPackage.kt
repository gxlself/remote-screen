package ai.gxlself.remotescreen.rsnative.localauth

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class LocalAuthPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == LocalAuthModule.NAME) LocalAuthModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      LocalAuthModule.NAME to ReactModuleInfo(
        LocalAuthModule.NAME,
        LocalAuthModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
