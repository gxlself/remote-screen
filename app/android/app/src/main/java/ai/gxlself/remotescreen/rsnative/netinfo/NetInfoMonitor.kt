package ai.gxlself.remotescreen.rsnative.netinfo

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import androidx.core.content.ContextCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import ai.gxlself.remotescreen.logic.netinfo.NetInfoCidr
import ai.gxlself.remotescreen.logic.netinfo.NetInfoClassifier
import ai.gxlself.remotescreen.logic.netinfo.NetInfoDebouncer
import ai.gxlself.remotescreen.logic.netinfo.NetInfoPath
import ai.gxlself.remotescreen.logic.netinfo.NetInfoRawAddress
import ai.gxlself.remotescreen.logic.netinfo.NetInfoRawIface
import ai.gxlself.remotescreen.logic.netinfo.NetInfoSnapshot
import ai.gxlself.remotescreen.logic.netinfo.NetPathMapper
import ai.gxlself.remotescreen.logic.netinfo.WorkProfileVpn
import ai.gxlself.remotescreen.rsnative.common.RsLog
import java.net.NetworkInterface
import java.net.SocketException

/**
 * Owns the Android network callbacks and the native network snapshot.
 *
 * All mutable state and all reads of Android network state are serialized on
 * the handler thread. This keeps callback bursts, interface enumeration and
 * pool updates from racing one another.
 */
class NetInfoMonitor private constructor(context: Context) : DefaultLifecycleObserver {
  fun interface Listener {
    fun onChange(snapshot: NetInfoSnapshot, sequence: Long)
  }

  private val appContext = context.applicationContext
  private val connectivityManager =
    appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
  private val thread = HandlerThread("rs-netinfo").also { it.start() }
  private val handler = Handler(thread.looper)
  private val mainHandler = Handler(Looper.getMainLooper())
  private val debouncer = NetInfoDebouncer()
  private val pollRunnable = object : Runnable {
    override fun run() {
      if (!started || listener == null || !foreground) return
      refreshAndNotify()
      handler.postDelayed(this, POLL_INTERVAL_MS)
    }
  }

  private val defaultCallback = object : ConnectivityManager.NetworkCallback() {
    override fun onAvailable(network: Network) = callbackRefresh()
    override fun onLost(network: Network) = callbackRefresh()
    override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) =
      callbackRefresh()
    override fun onLinkPropertiesChanged(
      network: Network,
      linkProperties: android.net.LinkProperties,
    ) = callbackRefresh()
  }

  private val vpnCallback = object : ConnectivityManager.NetworkCallback() {
    override fun onAvailable(network: Network) = callbackRefresh()
    override fun onLost(network: Network) = callbackRefresh()
    override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) =
      callbackRefresh()
    override fun onLinkPropertiesChanged(
      network: Network,
      linkProperties: android.net.LinkProperties,
    ) = callbackRefresh()
  }

  private val restrictBackgroundReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
      if (ConnectivityManager.ACTION_RESTRICT_BACKGROUND_CHANGED == intent.action) {
        callbackRefresh()
      }
    }
  }

  @Volatile private var lifecycleRegistered = false
  @Volatile private var started = false
  private var foreground = false
  private var defaultCallbackRegistered = false
  private var vpnCallbackRegistered = false
  private var receiverRegistered = false
  private var listener: Listener? = null
  private var vpnPools: List<NetInfoCidr> = emptyList()
  private var lastSnapshot: NetInfoSnapshot? = null
  private var sequence = 0L
  private var refreshGeneration = 0L

  fun start(newListener: Listener) {
    handler.post {
      listener = newListener
      if (started) return@post
      started = true
      foreground = false
      registerLifecycle()
      registerNetworkSources()
      scheduleRefresh()
      updatePolling()
    }
  }

  fun stop() {
    handler.post {
      listener = null
      started = false
      refreshGeneration += 1
      lastSnapshot = null
      handler.removeCallbacks(pollRunnable)
      unregisterNetworkSources()
      unregisterLifecycle()
    }
  }

  fun setVpnPools(values: List<String>) {
    handler.post {
      vpnPools = values.mapNotNull { NetInfoCidr.parse(it) }
      if (started) refreshAndNotify()
    }
  }

  fun getStatus(callback: (NetInfoSnapshot, Long) -> Unit) {
    handler.post {
      callback(readSnapshot(), sequence)
    }
  }

  private fun registerLifecycle() {
    if (lifecycleRegistered) return
    lifecycleRegistered = true
    mainHandler.post {
      if (started) {
        ProcessLifecycleOwner.get().lifecycle.addObserver(this)
      } else {
        lifecycleRegistered = false
      }
    }
  }

  private fun unregisterLifecycle() {
    if (!lifecycleRegistered) return
    lifecycleRegistered = false
    mainHandler.post {
      ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
    }
  }

  override fun onStart(owner: LifecycleOwner) {
    handler.post {
      if (!started) return@post
      foreground = true
      updatePolling()
    }
  }

  override fun onStop(owner: LifecycleOwner) {
    handler.post {
      foreground = false
      handler.removeCallbacks(pollRunnable)
    }
  }

  private fun registerNetworkSources() {
    try {
      connectivityManager.registerDefaultNetworkCallback(defaultCallback, handler)
      defaultCallbackRegistered = true
    } catch (error: RuntimeException) {
      logError("defaultCallback", error)
    }

    try {
      val request = NetworkRequest.Builder()
        .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
        .build()
      connectivityManager.registerNetworkCallback(request, vpnCallback, handler)
      vpnCallbackRegistered = true
    } catch (error: RuntimeException) {
      logError("vpnCallback", error)
    }

    try {
      val filter = IntentFilter(ConnectivityManager.ACTION_RESTRICT_BACKGROUND_CHANGED)
      ContextCompat.registerReceiver(
        appContext,
        restrictBackgroundReceiver,
        filter,
        ContextCompat.RECEIVER_NOT_EXPORTED,
      )
      receiverRegistered = true
    } catch (error: RuntimeException) {
      logError("restrictBackgroundReceiver", error)
    }
  }

  private fun unregisterNetworkSources() {
    if (defaultCallbackRegistered) {
      try {
        connectivityManager.unregisterNetworkCallback(defaultCallback)
      } catch (error: RuntimeException) {
        logError("defaultCallbackUnregister", error)
      }
      defaultCallbackRegistered = false
    }
    if (vpnCallbackRegistered) {
      try {
        connectivityManager.unregisterNetworkCallback(vpnCallback)
      } catch (error: RuntimeException) {
        logError("vpnCallbackUnregister", error)
      }
      vpnCallbackRegistered = false
    }
    if (receiverRegistered) {
      try {
        appContext.unregisterReceiver(restrictBackgroundReceiver)
      } catch (error: RuntimeException) {
        logError("restrictBackgroundReceiverUnregister", error)
      }
      receiverRegistered = false
    }
  }

  private fun callbackRefresh() {
    handler.post { if (started) scheduleRefresh() }
  }

  private fun scheduleRefresh() {
    refreshGeneration += 1
    val generation = refreshGeneration
    debouncer.update(nowUs())
    handler.postDelayed({
      if (!started || generation != refreshGeneration) return@postDelayed
      debouncer.fireIfDue(nowUs()) { refreshAndNotify() }
    }, DEBOUNCE_MS)
  }

  private fun updatePolling() {
    handler.removeCallbacks(pollRunnable)
    if (started && listener != null && foreground) {
      handler.postDelayed(pollRunnable, POLL_INTERVAL_MS)
    }
  }

  private fun refreshAndNotify() {
    val snapshot = readSnapshot()
    if (snapshot == lastSnapshot) return
    lastSnapshot = snapshot
    sequence += 1
    val currentListener = listener ?: return
    logSnapshot(snapshot, sequence)
    currentListener.onChange(snapshot, sequence)
  }

  private fun readSnapshot(): NetInfoSnapshot {
    val (path, appVpnIface) = readPath()
    val classified = NetInfoClassifier.snapshot(readInterfaces(), vpnPools)
    return WorkProfileVpn.mark(NetInfoSnapshot(classified.ifaces, path), appVpnIface)
  }

  private fun readInterfaces(): List<NetInfoRawIface> {
    val interfaces = try {
      NetworkInterface.getNetworkInterfaces()
    } catch (error: SocketException) {
      logError("interfaces", error)
      return emptyList()
    }
    if (interfaces == null) {
      RsLog.marker("NETINFO", mapOf("interfaces" to "null"))
      return emptyList()
    }

    val result = mutableListOf<NetInfoRawIface>()
    try {
      while (interfaces.hasMoreElements()) {
        val networkInterface = interfaces.nextElement()
        val addresses = if (!networkInterface.isUp) {
          emptyList()
        } else {
          networkInterface.interfaceAddresses
            .mapNotNull { address ->
              val hostAddress = address.address?.hostAddress ?: return@mapNotNull null
              NetInfoRawAddress(
                WorkProfileVpn.stripZone(hostAddress),
                address.networkPrefixLength.toInt(),
              )
            }
            .sortedWith(compareBy<NetInfoRawAddress> { it.address }.thenBy { it.prefixLength })
        }
        result += NetInfoRawIface(networkInterface.name, addresses)
      }
    } catch (error: SocketException) {
      logError("interfaces", error)
      return emptyList()
    }
    return result
  }

  private fun readPath(): Pair<NetInfoPath, String?> {
    val activeNetwork = connectivityManager.activeNetwork
    val capabilities = activeNetwork?.let(connectivityManager::getNetworkCapabilities)
    val transports = buildSet {
      val knownTransports = intArrayOf(
        NetworkCapabilities.TRANSPORT_CELLULAR,
        NetworkCapabilities.TRANSPORT_WIFI,
        NetworkCapabilities.TRANSPORT_BLUETOOTH,
        NetworkCapabilities.TRANSPORT_ETHERNET,
        NetworkCapabilities.TRANSPORT_VPN,
        NetworkCapabilities.TRANSPORT_WIFI_AWARE,
        NetworkCapabilities.TRANSPORT_LOWPAN,
        NetworkCapabilities.TRANSPORT_USB,
        NetworkCapabilities.TRANSPORT_THREAD,
        NetworkCapabilities.TRANSPORT_SATELLITE,
      )
      knownTransports.forEach { transport ->
        if (capabilities?.hasTransport(transport) == true) add(transport)
      }
    }
    val path = NetPathMapper.path(
      hasNetwork = activeNetwork != null,
      hasInternet = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true,
      transports = transports,
      notMetered = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) == true,
      dataSaver = connectivityManager.restrictBackgroundStatus ==
        ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED,
    )
    val appVpnIface = if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true) {
      activeNetwork?.let(connectivityManager::getLinkProperties)?.interfaceName
    } else {
      null
    }
    return path to appVpnIface
  }

  private fun logSnapshot(snapshot: NetInfoSnapshot, sequence: Long) {
    RsLog.marker(
      "NETINFO",
      mapOf(
        "seq" to sequence,
        "ifaces" to snapshot.ifaces.map { it.name },
        "vpn" to snapshot.ifaces.filter { it.isVpn }.map { it.name },
        "workProfile" to snapshot.ifaces.filter { it.workProfile }.map { it.name },
        "path" to (snapshot.path.interfaceTypes.joinToString("|").ifEmpty { snapshot.path.status }),
      ),
    )
  }

  private fun logError(source: String, error: Throwable) {
    RsLog.marker(
      "NETINFO",
      mapOf("error" to source, "type" to error::class.java.simpleName),
    )
  }

  private fun nowUs(): Long = SystemClock.elapsedRealtimeNanos() / NANOS_PER_MICRO

  companion object {
    private const val DEBOUNCE_MS = 500L
    private const val POLL_INTERVAL_MS = 10_000L
    private const val NANOS_PER_MICRO = 1_000L

    @Volatile private var instance: NetInfoMonitor? = null

    fun get(context: Context): NetInfoMonitor =
      instance ?: synchronized(this) {
        instance ?: NetInfoMonitor(context).also { instance = it }
      }
  }
}
