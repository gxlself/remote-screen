package ai.gxlself.remotescreen.rsnative.files

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

/** Registers the Android RsFiles TurboModule. */
class RsFilesPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsFilesModule.NAME) RsFilesModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsFilesModule.NAME to ReactModuleInfo(
        RsFilesModule.NAME,
        RsFilesModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
