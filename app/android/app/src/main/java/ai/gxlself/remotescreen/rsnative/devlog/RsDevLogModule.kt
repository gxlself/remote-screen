package ai.gxlself.remotescreen.rsnative.devlog

import android.util.Log
import ai.gxlself.remotescreen.specs.NativeRsDevLogSpec
import com.facebook.react.bridge.ReactApplicationContext

class RsDevLogModule(reactContext: ReactApplicationContext) : NativeRsDevLogSpec(reactContext) {
  override fun getName(): String = NAME

  override fun log(line: String) {
    val maxLineLength = 3_800
    val output = if (line.length > maxLineLength) {
      line.take(maxLineLength) + "…(+${line.length - maxLineLength})"
    } else {
      line
    }
    Log.i("RS", output)
  }

  companion object {
    const val NAME = "RsDevLog"
  }
}
