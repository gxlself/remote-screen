package ai.gxlself.remotescreen.rsnative.system

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class RsSystemPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsSystemModule.NAME) RsSystemModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsSystemModule.NAME to ReactModuleInfo(
        RsSystemModule.NAME,
        RsSystemModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
