package ai.gxlself.remotescreen

import android.app.Application
import ai.gxlself.remotescreen.rsnative.androidsession.RsAndroidSessionPackage
import ai.gxlself.remotescreen.rsnative.audio.RsAudioPackage
import ai.gxlself.remotescreen.rsnative.bonjour.RsBonjourPackage
import ai.gxlself.remotescreen.rsnative.bootstrap.RsBootstrap
import ai.gxlself.remotescreen.rsnative.devlog.RsDevLogPackage
import ai.gxlself.remotescreen.rsnative.files.RsFilesPackage
import ai.gxlself.remotescreen.rsnative.keyboard.RsKeyCapturePackage
import ai.gxlself.remotescreen.rsnative.lifecycle.RsLifecyclePackage
import ai.gxlself.remotescreen.rsnative.localauth.LocalAuthPackage
import ai.gxlself.remotescreen.rsnative.netinfo.NetInfoPlusPackage
import ai.gxlself.remotescreen.rsnative.scanner.RsQrScannerPackage
import ai.gxlself.remotescreen.rsnative.securekey.SecureKeyPackage
import ai.gxlself.remotescreen.rsnative.session.RemoteSessionPackage
import ai.gxlself.remotescreen.rsnative.system.RsSystemPackage
import ai.gxlself.remotescreen.rsnative.terminal.RsTerminalPackage
import com.facebook.react.PackageList
import com.facebook.react.ReactApplication
import com.facebook.react.ReactHost
import com.facebook.react.ReactNativeApplicationEntryPoint.loadReactNative
import com.facebook.react.defaults.DefaultReactHost.getDefaultReactHost

class MainApplication : Application(), ReactApplication {

  override val reactHost: ReactHost by lazy {
    getDefaultReactHost(
      context = applicationContext,
      packageList =
        PackageList(this).packages.apply {
          add(RsDevLogPackage())
          add(RsSystemPackage())
          add(NetInfoPlusPackage())
          add(LocalAuthPackage())
          add(SecureKeyPackage())
          add(RsLifecyclePackage())
          add(RsQrScannerPackage())
          add(RsKeyCapturePackage())
          add(RemoteSessionPackage())
          add(RsTerminalPackage())
          add(RsAudioPackage())
          add(RsBonjourPackage())
          add(RsAndroidSessionPackage())
          add(RsFilesPackage())
        },
    )
  }

  override fun onCreate() {
    super.onCreate()
    loadReactNative(this)
    RsBootstrap.start(this)
  }
}
