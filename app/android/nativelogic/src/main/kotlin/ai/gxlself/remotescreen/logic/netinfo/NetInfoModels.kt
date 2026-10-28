package ai.gxlself.remotescreen.logic.netinfo

data class NetInfoRawAddress(val address: String, val prefixLength: Int)

data class NetInfoRawIface(val name: String, val addrs: List<NetInfoRawAddress>)

data class NetInfoIface(
    val name: String,
    val addrs: List<String>,
    val isVpn: Boolean,
    val workProfile: Boolean = false,
)

data class NetInfoPath(
    val status: String,
    val interfaceTypes: List<String>,
    val expensive: Boolean,
    val constrained: Boolean,
) {
    companion object {
        val UNKNOWN = NetInfoPath("unsatisfied", emptyList(), expensive = false, constrained = false)
    }
}

@ExposedCopyVisibility
data class NetInfoSnapshot private constructor(
    val ifaces: List<NetInfoIface>,
    val path: NetInfoPath,
    @Suppress("unused") private val sortedMarker: Unit,
) {
    constructor(ifaces: List<NetInfoIface>, path: NetInfoPath = NetInfoPath.UNKNOWN) :
        this(ifaces.sortedBy(NetInfoIface::name), path, Unit)
}
