package ai.gxlself.remotescreen.rsnative.androidsession

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class RsAndroidSessionPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsAndroidSessionModule.NAME) RsAndroidSessionModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsAndroidSessionModule.NAME to ReactModuleInfo(
        RsAndroidSessionModule.NAME,
        RsAndroidSessionModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
