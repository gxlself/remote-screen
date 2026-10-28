package ai.gxlself.remotescreen.rsnative.audio

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

/** Registers the codegen TurboModule implementation. */
class RsAudioPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == RsAudioModule.NAME) RsAudioModule(reactContext) else null

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      RsAudioModule.NAME to ReactModuleInfo(
        RsAudioModule.NAME,
        RsAudioModule::class.java.name,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
