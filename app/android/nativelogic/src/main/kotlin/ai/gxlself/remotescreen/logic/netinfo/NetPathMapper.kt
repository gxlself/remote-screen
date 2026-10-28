package ai.gxlself.remotescreen.logic.netinfo

object NetPathMapper {
    private const val TRANSPORT_CELLULAR = 0 // NetworkCapabilities.TRANSPORT_CELLULAR
    private const val TRANSPORT_WIFI = 1 // NetworkCapabilities.TRANSPORT_WIFI
    private const val TRANSPORT_BLUETOOTH = 2 // NetworkCapabilities.TRANSPORT_BLUETOOTH
    private const val TRANSPORT_ETHERNET = 3 // NetworkCapabilities.TRANSPORT_ETHERNET
    private const val TRANSPORT_VPN = 4 // NetworkCapabilities.TRANSPORT_VPN
    private const val TRANSPORT_WIFI_AWARE = 5 // NetworkCapabilities.TRANSPORT_WIFI_AWARE
    private const val TRANSPORT_LOWPAN = 6 // NetworkCapabilities.TRANSPORT_LOWPAN
    private const val TRANSPORT_USB = 8 // NetworkCapabilities.TRANSPORT_USB
    private const val TRANSPORT_THREAD = 9 // NetworkCapabilities.TRANSPORT_THREAD
    private const val TRANSPORT_SATELLITE = 10 // NetworkCapabilities.TRANSPORT_SATELLITE

    private val otherTransports = setOf(
        TRANSPORT_BLUETOOTH,
        TRANSPORT_VPN,
        TRANSPORT_WIFI_AWARE,
        TRANSPORT_LOWPAN,
        TRANSPORT_THREAD,
        TRANSPORT_SATELLITE,
    )

    fun path(
        hasNetwork: Boolean,
        hasInternet: Boolean,
        transports: Set<Int>,
        notMetered: Boolean,
        dataSaver: Boolean,
    ): NetInfoPath {
        val interfaceTypes = buildList {
            if (TRANSPORT_WIFI in transports) add("wifi")
            if (TRANSPORT_CELLULAR in transports) add("cellular")
            if (TRANSPORT_ETHERNET in transports || TRANSPORT_USB in transports) add("wiredEthernet")
            if (transports.any { it in otherTransports || it !in knownTransports }) add("other")
        }
        return NetInfoPath(
            status = if (hasNetwork && hasInternet) "satisfied" else "unsatisfied",
            interfaceTypes = interfaceTypes,
            expensive = hasNetwork && !notMetered,
            constrained = dataSaver,
        )
    }

    private val knownTransports = setOf(
        TRANSPORT_CELLULAR,
        TRANSPORT_WIFI,
        TRANSPORT_BLUETOOTH,
        TRANSPORT_ETHERNET,
        TRANSPORT_VPN,
        TRANSPORT_WIFI_AWARE,
        TRANSPORT_LOWPAN,
        TRANSPORT_USB,
        TRANSPORT_THREAD,
        TRANSPORT_SATELLITE,
    )
}
