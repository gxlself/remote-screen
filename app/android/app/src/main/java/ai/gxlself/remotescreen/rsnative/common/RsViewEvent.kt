package ai.gxlself.remotescreen.rsnative.common

import android.view.View
import com.facebook.react.bridge.ReactContext
import com.facebook.react.bridge.WritableMap
import com.facebook.react.uimanager.UIManagerHelper
import com.facebook.react.uimanager.events.Event

class RsViewEvent(
  surfaceId: Int,
  viewId: Int,
  private val name: String,
  private val data: WritableMap?,
) : Event<RsViewEvent>(surfaceId, viewId) {
  override fun getEventName(): String = name

  public override fun getEventData(): WritableMap? = data
}

/**
 * Fabric event helpers. Example: `RsViewEvents.emit(view, "topGesture", payload)`.
 */
object RsViewEvents {
  /** Emits a Fabric event from any calling thread when the view is React-managed. */
  fun emit(view: View, name: String, data: WritableMap?) {
    if (view.id == View.NO_ID) return
    val context = view.context as? ReactContext ?: return
    val dispatcher = UIManagerHelper.getEventDispatcherForReactTag(context, view.id) ?: return
    dispatcher.dispatchEvent(RsViewEvent(UIManagerHelper.getSurfaceId(view), view.id, name, data))
  }

  /** Returns the registration map expected by `getExportedCustomDirectEventTypeConstants()`. */
  fun directEventTypes(vararg names: String): MutableMap<String, Any> =
    names.associateWith { name ->
      val eventName = name.removePrefix("top")
      mapOf("registrationName" to "on$eventName")
    }.toMutableMap()
}
