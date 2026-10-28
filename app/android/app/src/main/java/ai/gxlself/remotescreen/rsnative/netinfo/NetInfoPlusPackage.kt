package ai.gxlself.remotescreen.rsnative.netinfo

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class NetInfoPlusPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == NetInfoPlusModule.NAME) NetInfoPlusModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      NetInfoPlusModule.NAME to ReactModuleInfo(
        NetInfoPlusModule.NAME,
        NetInfoPlusModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
