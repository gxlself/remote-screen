package ai.gxlself.remotescreen.rsnative.common

import android.util.Log
import ai.gxlself.remotescreen.logic.common.MarkerLine

/** Emits one machine-readable marker line to the shared Android logcat tag. */
object RsLog {
  fun marker(tag: String, fields: Map<String, Any?>) {
    Log.i("RS", MarkerLine.format(tag, fields))
  }
}
