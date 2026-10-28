package ai.gxlself.remotescreen.logic.netinfo

object WorkProfileVpn {
    private val tunnelPrefixes = listOf("utun", "ipsec", "ppp", "tun", "tap", "wg")

    fun stripZone(address: String): String = address.substringBefore('%')

    fun isTunnelName(name: String): Boolean = tunnelPrefixes.any(name::startsWith)

    fun mark(snapshot: NetInfoSnapshot, appVpnIface: String?): NetInfoSnapshot =
        NetInfoSnapshot(
            ifaces = snapshot.ifaces.map { iface ->
                iface.copy(
                    workProfile = iface.isVpn && isTunnelName(iface.name) && iface.name != appVpnIface,
                )
            },
            path = snapshot.path,
        )
}
