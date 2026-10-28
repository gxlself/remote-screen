package ai.gxlself.remotescreen

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import androidx.appcompat.app.AppCompatDelegate
import ai.gxlself.remotescreen.logic.launch.LaunchArgs
import ai.gxlself.remotescreen.logic.launch.LaunchDataDir
import ai.gxlself.remotescreen.rsnative.common.RsActivityHooks
import ai.gxlself.remotescreen.rsnative.common.RsLog
import ai.gxlself.remotescreen.rsnative.files.RsFilesIntents
import com.facebook.react.ReactActivity
import com.facebook.react.ReactActivityDelegate
import com.facebook.react.defaults.DefaultNewArchitectureEntryPoint.fabricEnabled
import com.facebook.react.defaults.DefaultReactActivityDelegate
import com.swmansion.rnscreens.fragment.restoration.RNScreensFragmentFactory
import java.io.IOException

class MainActivity : ReactActivity() {

  /**
   * Returns the name of the main component registered from JavaScript. This is used to schedule
   * rendering of the component.
   */
  override fun getMainComponentName(): String = "RemoteScreen"

  override fun onCreate(savedInstanceState: Bundle?) {
    rewriteIncomingShareIntent()
    // react-native-screens：进程被回收后恢复时不重建屏幕 Fragment（README「Android」一节）
    supportFragmentManager.fragmentFactory = RNScreensFragmentFactory()
    // 只有深色主题：让 uiMode 恒为夜间，RN edge-to-edge 据此把导航栏图标设成浅色
    AppCompatDelegate.setDefaultNightMode(AppCompatDelegate.MODE_NIGHT_YES)
    super.onCreate(savedInstanceState)
  }

  /**
   * Returns the instance of the [ReactActivityDelegate]. We use [DefaultReactActivityDelegate]
   * which allows you to enable New Architecture with a single boolean flags [fabricEnabled]
   */
  override fun createReactActivityDelegate(): ReactActivityDelegate =
    object : DefaultReactActivityDelegate(this, mainComponentName, fabricEnabled) {
      override fun getLaunchOptions(): Bundle {
        val options = Bundle()
        val extras = intent?.extras
        val values = linkedMapOf<String, String?>()
        for (key in LaunchArgs.VALUE_KEYS) {
          values[key] = extras?.stringExtraOrNull(key)
        }
        LaunchArgs.properties(values).forEach { (key, value) -> options.putString(key, value) }

        val dataDir = LaunchDataDir.dir(noBackupFilesDir)
        try {
          if (!dataDir.exists() && !dataDir.mkdirs()) {
            throw IOException("Unable to create ${dataDir.absolutePath}")
          }
          if (!dataDir.isDirectory) {
            throw IOException("Data directory is not a directory: ${dataDir.absolutePath}")
          }
          options.putString("dataDir", dataDir.absolutePath)
        } catch (error: Exception) {
          RsLog.marker(
            "BOOT",
            mapOf("dataDir" to "error", "message" to (error.message ?: error::class.java.simpleName)),
          )
        }

        RsLog.marker("LAUNCH", mapOf("keys" to options.keySet().toList().sorted()))
        return options
      }
    }

  override fun onUserLeaveHint() {
    super.onUserLeaveHint()
    RsActivityHooks.dispatchUserLeaveHint(this)
  }

  override fun onPictureInPictureModeChanged(inPip: Boolean, newConfig: Configuration) {
    super.onPictureInPictureModeChanged(inPip, newConfig)
    RsActivityHooks.dispatchPipChanged(this, inPip)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    RsActivityHooks.dispatchNewIntent(this, intent)
  }

  override fun onResume() {
    super.onResume()
    RsActivityHooks.setCurrent(this)
  }

  override fun onPause() {
    super.onPause()
    RsActivityHooks.setCurrent(null)
  }

  private fun Bundle.stringExtraOrNull(key: String): String? =
    try {
      getString(key)
    } catch (_: ClassCastException) {
      null
    }

  private fun rewriteIncomingShareIntent() {
    val incoming = intent ?: return
    if (!RsFilesIntents.isShareAction(incoming)) {
      return
    }
    val first = RsFilesIntents.streamUris(incoming).firstOrNull() ?: return
    val readGrant = incoming.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION
    incoming.action = Intent.ACTION_VIEW
    incoming.data = first
    // Keep the temporary provider grant while Linking/importIncoming consumes it.
    incoming.addFlags(readGrant)
    setIntent(incoming)
  }
}
