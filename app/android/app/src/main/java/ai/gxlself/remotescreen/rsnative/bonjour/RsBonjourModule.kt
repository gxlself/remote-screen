package ai.gxlself.remotescreen.rsnative.bonjour

import ai.gxlself.remotescreen.logic.bonjour.BonjourLogic
import ai.gxlself.remotescreen.logic.bonjour.BonjourResolveQueue
import ai.gxlself.remotescreen.logic.bonjour.BonjourService
import ai.gxlself.remotescreen.logic.bonjour.BonjourServiceDeduplicator
import ai.gxlself.remotescreen.specs.NativeRsBonjourSpec
import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.annotation.RequiresApi
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReactApplicationContext
import java.util.concurrent.Executor

/** Android Bonjour browser backed by NsdManager. */
class RsBonjourModule(
  reactContext: ReactApplicationContext,
) : NativeRsBonjourSpec(reactContext) {
  private val nsdManager = reactContext.getSystemService(Context.NSD_SERVICE) as? NsdManager
  private val wifiManager = reactContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
  private val callbackExecutor = Executor { command ->
    Handler(Looper.getMainLooper()).post(command)
  }

  private var multicastLock: WifiManager.MulticastLock? = null
  private var discoveryListener: NsdManager.DiscoveryListener? = null
  private var resolveListener: NsdManager.ResolveListener? = null
  private val serviceInfoCallbacks = mutableMapOf<String, NsdManager.ServiceInfoCallback>()
  private val serviceInfos = mutableMapOf<String, NsdServiceInfo>()
  private val discoveredServices = mutableSetOf<String>()
  private val resolveQueue = BonjourResolveQueue<String>()
  private val emittedServices = BonjourServiceDeduplicator<String>()
  private var operationId = 0L

  override fun start(type: String) {
    if (type != SERVICE_TYPE) return

    stop()
    val manager = nsdManager ?: return
    val operation = operationId
    acquireMulticastLock()

    val listener = createDiscoveryListener(operation)
    discoveryListener = listener
    try {
      manager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    } catch (_: RuntimeException) {
      stopInternal(stopDiscovery = false)
    }
  }

  override fun stop() {
    stopInternal(stopDiscovery = true)
  }

  override fun invalidate() {
    stop()
    super.invalidate()
  }

  private fun createDiscoveryListener(operation: Long): NsdManager.DiscoveryListener =
    object : NsdManager.DiscoveryListener {
      override fun onDiscoveryStarted(serviceType: String) = Unit

      override fun onDiscoveryStopped(serviceType: String) {
        if (operation != operationId) return
        stopInternal(stopDiscovery = false)
      }

      override fun onServiceFound(serviceInfo: NsdServiceInfo) {
        if (operation != operationId) return
        handleServiceFound(serviceInfo, operation)
      }

      override fun onServiceLost(serviceInfo: NsdServiceInfo) {
        if (operation != operationId) return
        val name = serviceInfo.serviceName ?: return
        handleServiceLost(name, serviceInfo)
      }

      override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
        if (operation == operationId) stopInternal(stopDiscovery = false)
      }

      override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
        if (operation == operationId) stopInternal(stopDiscovery = false)
      }
    }

  private fun handleServiceFound(serviceInfo: NsdServiceInfo, operation: Long) {
    if (!isRfbService(serviceInfo)) return
    val name = serviceInfo.serviceName ?: return
    if (name.isBlank() || !discoveredServices.add(name)) return

    serviceInfos[name] = serviceInfo
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      registerServiceInfoCallback(name, serviceInfo, operation)
    } else if (resolveQueue.enqueue(name)) {
      resolveNext(operation)
    }
  }

  @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
  private fun registerServiceInfoCallback(
    name: String,
    serviceInfo: NsdServiceInfo,
    operation: Long,
  ) {
    val callback = object : NsdManager.ServiceInfoCallback {
      override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
        if (operation != operationId) return
        if (serviceInfoCallbacks[name] === this) serviceInfoCallbacks.remove(name)
      }

      override fun onServiceInfoCallbackUnregistered() = Unit

      override fun onServiceLost() {
        if (operation == operationId) handleServiceLost(name, serviceInfo)
      }

      override fun onServiceUpdated(updatedInfo: NsdServiceInfo) {
        if (operation == operationId) handleServiceResolved(name, updatedInfo)
      }
    }
    serviceInfoCallbacks[name] = callback
    try {
      nsdManager?.registerServiceInfoCallback(serviceInfo, callbackExecutor, callback)
    } catch (_: RuntimeException) {
      if (serviceInfoCallbacks[name] === callback) serviceInfoCallbacks.remove(name)
    }
  }

  @Suppress("DEPRECATION")
  private fun resolveNext(operation: Long) {
    if (operation != operationId || Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      return
    }
    val name = resolveQueue.startNext() ?: return
    val serviceInfo = serviceInfos[name]
    if (serviceInfo == null) {
      resolveQueue.complete(name)
      resolveNext(operation)
      return
    }

    val listener = object : NsdManager.ResolveListener {
      override fun onServiceResolved(resolvedInfo: NsdServiceInfo) {
        finishResolve(operation, name, this, resolvedInfo)
      }

      override fun onResolveFailed(failedInfo: NsdServiceInfo, errorCode: Int) {
        finishResolve(operation, name, this, null)
      }
    }
    resolveListener = listener
    try {
      nsdManager?.resolveService(serviceInfo, listener)
    } catch (_: RuntimeException) {
      finishResolve(operation, name, listener, null)
    }
  }

  private fun finishResolve(
    operation: Long,
    name: String,
    listener: NsdManager.ResolveListener,
    resolvedInfo: NsdServiceInfo?,
  ) {
    if (operation != operationId || resolveListener !== listener) return
    resolveListener = null
    if (!resolveQueue.complete(name)) return

    if (resolvedInfo != null && discoveredServices.contains(name)) {
      serviceInfos[name] = resolvedInfo
      handleServiceResolved(name, resolvedInfo)
    }
    resolveNext(operation)
  }

  private fun handleServiceResolved(name: String, serviceInfo: NsdServiceInfo) {
    if (!discoveredServices.contains(name)) return
    serviceInfos[name] = serviceInfo
    val service = toBonjourService(name, serviceInfo)
    if (emittedServices.shouldEmitFound(name, service)) {
      emitService("found", service)
    }
  }

  private fun handleServiceLost(name: String, fallbackInfo: NsdServiceInfo?) {
    if (!discoveredServices.remove(name)) return

    val callback = serviceInfoCallbacks.remove(name)
    if (callback != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      try {
        unregisterServiceInfoCallback(callback)
      } catch (_: RuntimeException) {
        // The system may have already unregistered a callback after reporting a loss.
      }
    }
    resolveQueue.removePending(name)
    val serviceInfo = serviceInfos.remove(name)
    val service = emittedServices.remove(name)
      ?: toBonjourService(name, serviceInfo ?: fallbackInfo)
    emitService("lost", service)
  }

  private fun toBonjourService(name: String, serviceInfo: NsdServiceInfo?): BonjourService =
    BonjourService(
      name = serviceInfo?.serviceName?.takeIf(String::isNotBlank) ?: name,
      host = BonjourLogic.hostForServiceName(
        serviceInfo?.serviceName?.takeIf(String::isNotBlank) ?: name,
      ),
      port = serviceInfo?.port ?: 0,
      addrs = serviceInfo?.let(::addressStrings).orEmpty(),
    )

  private fun addressStrings(serviceInfo: NsdServiceInfo): List<String> {
    val allAddresses = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      serviceInfo.hostAddresses.mapNotNull { it.hostAddress }
    } else {
      emptyList()
    }
    @Suppress("DEPRECATION")
    val legacyAddress = serviceInfo.host?.hostAddress
    return BonjourLogic.sortAndLimitAddresses(allAddresses + listOfNotNull(legacyAddress))
  }

  private fun emitService(kind: String, service: BonjourService) {
    val serviceMap = Arguments.createMap().apply {
      putString("name", service.name)
      putString("host", service.host)
      putInt("port", service.port)
      putArray("addrs", Arguments.createArray().apply {
        service.addrs.forEach(::pushString)
      })
    }
    emitOnService(Arguments.createMap().apply {
      putString("kind", kind)
      putMap("service", serviceMap)
    })
  }

  private fun isRfbService(serviceInfo: NsdServiceInfo): Boolean {
    val type = serviceInfo.serviceType ?: return false
    return type == SERVICE_TYPE || type == "$SERVICE_TYPE."
  }

  private fun acquireMulticastLock() {
    val manager = wifiManager ?: return
    try {
      multicastLock = manager.createMulticastLock(MULTICAST_LOCK_TAG).also { lock ->
        lock.setReferenceCounted(false)
        lock.acquire()
      }
    } catch (_: RuntimeException) {
      multicastLock = null
    }
  }

  private fun stopInternal(stopDiscovery: Boolean) {
    operationId += 1
    val manager = nsdManager
    val listener = discoveryListener
    discoveryListener = null
    if (stopDiscovery && listener != null) {
      try {
        manager?.stopServiceDiscovery(listener)
      } catch (_: RuntimeException) {
        // Stop is idempotent from the JavaScript caller's perspective.
      }
    }

    val callbacks = serviceInfoCallbacks.values.toList()
    serviceInfoCallbacks.clear()
    if (manager != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      callbacks.forEach { callback ->
        try {
          unregisterServiceInfoCallback(callback)
        } catch (_: RuntimeException) {
          // Registration can fail before the corresponding unregister call.
        }
      }
    }

    val activeResolveListener = resolveListener
    resolveListener = null
    if (manager != null && activeResolveListener != null &&
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE
    ) {
      try {
        stopServiceResolution(activeResolveListener)
      } catch (_: RuntimeException) {
        // The operation may have completed while stop was racing its callback.
      }
    }

    resolveQueue.clear()
    serviceInfos.clear()
    discoveredServices.clear()
    emittedServices.clear()
    releaseMulticastLock()
  }

  @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
  private fun unregisterServiceInfoCallback(callback: NsdManager.ServiceInfoCallback) {
    nsdManager?.unregisterServiceInfoCallback(callback)
  }

  @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
  private fun stopServiceResolution(listener: NsdManager.ResolveListener) {
    nsdManager?.stopServiceResolution(listener)
  }

  private fun releaseMulticastLock() {
    val lock = multicastLock ?: return
    multicastLock = null
    try {
      if (lock.isHeld) lock.release()
    } catch (_: RuntimeException) {
      // A process shutdown can release the lock before invalidate() runs.
    }
  }

  companion object {
    const val NAME = "RsBonjour"
    private const val SERVICE_TYPE = "_rfb._tcp"
    private const val MULTICAST_LOCK_TAG = "RemoteScreenBonjour"
  }
}
