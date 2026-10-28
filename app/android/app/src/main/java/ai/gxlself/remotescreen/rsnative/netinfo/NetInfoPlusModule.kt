package ai.gxlself.remotescreen.rsnative.netinfo

import ai.gxlself.remotescreen.logic.netinfo.NetInfoSnapshot
import ai.gxlself.remotescreen.specs.NativeNetInfoPlusSpec
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.WritableMap

class NetInfoPlusModule(
  private val reactContext: ReactApplicationContext,
) : NativeNetInfoPlusSpec(reactContext) {
  private val monitor = NetInfoMonitor.get(reactContext)

  override fun getName(): String = NAME

  override fun initialize() {
    super.initialize()
    monitor.start(NetInfoMonitor.Listener { snapshot, sequence ->
      emitOnChange(toWritableMap(snapshot, sequence))
    })
  }

  override fun invalidate() {
    monitor.stop()
    super.invalidate()
  }

  override fun getStatus(promise: Promise) {
    monitor.getStatus { snapshot, sequence ->
      promise.resolve(toWritableMap(snapshot, sequence))
    }
  }

  override fun setVpnPools(pools: ReadableArray) {
    val values = buildList {
      for (index in 0 until pools.size()) {
        val value = try {
          pools.getString(index)
        } catch (_: RuntimeException) {
          null
        }
        if (value != null) add(value)
      }
    }
    monitor.setVpnPools(values)
  }

  private fun toWritableMap(snapshot: NetInfoSnapshot, sequence: Long): WritableMap {
    val result = Arguments.createMap()
    val ifaceArray = Arguments.createArray()
    snapshot.ifaces.forEach { iface ->
      val ifaceMap = Arguments.createMap()
      ifaceMap.putString("name", iface.name)
      val addresses = Arguments.createArray()
      iface.addrs.forEach(addresses::pushString)
      ifaceMap.putArray("addrs", addresses)
      ifaceMap.putBoolean("isVpn", iface.isVpn)
      if (iface.workProfile) ifaceMap.putBoolean("workProfile", true)
      ifaceArray.pushMap(ifaceMap)
    }
    result.putArray("ifaces", ifaceArray)

    val path = Arguments.createMap()
    path.putString("status", snapshot.path.status)
    val interfaceTypes = Arguments.createArray()
    snapshot.path.interfaceTypes.forEach(interfaceTypes::pushString)
    path.putArray("interfaceTypes", interfaceTypes)
    path.putBoolean("expensive", snapshot.path.expensive)
    path.putBoolean("constrained", snapshot.path.constrained)
    result.putMap("path", path)
    result.putDouble("seq", sequence.toDouble())
    return result
  }

  companion object {
    const val NAME = "NetInfoPlus"
  }
}
