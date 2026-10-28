package ai.gxlself.remotescreen.rsnative.bonjour

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

/** Registers the codegen TurboModule implementation. */
class RsBonjourPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsBonjourModule.NAME) RsBonjourModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsBonjourModule.NAME to ReactModuleInfo(
        RsBonjourModule.NAME,
        RsBonjourModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
