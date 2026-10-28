package ai.gxlself.remotescreen.rsnative.devlog

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class RsDevLogPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsDevLogModule.NAME) RsDevLogModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsDevLogModule.NAME to ReactModuleInfo(
        RsDevLogModule.NAME,
        RsDevLogModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
