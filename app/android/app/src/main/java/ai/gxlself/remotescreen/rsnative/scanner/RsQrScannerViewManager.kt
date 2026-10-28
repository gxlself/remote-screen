package ai.gxlself.remotescreen.rsnative.scanner

import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.uimanager.SimpleViewManager
import com.facebook.react.uimanager.ThemedReactContext
import com.facebook.react.uimanager.ViewManagerDelegate
import com.facebook.react.viewmanagers.RsQrScannerViewManagerDelegate
import com.facebook.react.viewmanagers.RsQrScannerViewManagerInterface
import ai.gxlself.remotescreen.rsnative.common.RsViewEvents

@ReactModule(name = "RsQrScannerView")
class RsQrScannerViewManager : SimpleViewManager<RsQrScannerView>(),
  RsQrScannerViewManagerInterface<RsQrScannerView> {
  private val delegate: ViewManagerDelegate<RsQrScannerView> =
    RsQrScannerViewManagerDelegate<RsQrScannerView, RsQrScannerViewManager>(this)

  override fun getName(): String = "RsQrScannerView"

  override fun createViewInstance(reactContext: ThemedReactContext): RsQrScannerView =
    RsQrScannerView(reactContext)

  override fun getDelegate(): ViewManagerDelegate<RsQrScannerView> = delegate

  override fun setActive(view: RsQrScannerView, value: Boolean) {
    view.active = value
  }

  override fun setTorch(view: RsQrScannerView, value: Boolean) {
    view.torch = value
  }

  override fun getExportedCustomDirectEventTypeConstants(): MutableMap<String, Any> =
    RsViewEvents.directEventTypes("topCode", "topScannerState")

  override fun onDropViewInstance(view: RsQrScannerView) {
    view.dispose()
    super.onDropViewInstance(view)
  }
}
