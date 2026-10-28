package ai.gxlself.remotescreen.logic.netinfo

object NetInfoClassifier {
    private val linkLocalRanges = listOfNotNull(
        NetInfoCidr.parse("169.254.0.0/16"),
        NetInfoCidr.parse("fe80::/10"),
    )
    private val loopbackRanges = listOfNotNull(
        NetInfoCidr.parse("127.0.0.0/8"),
        NetInfoCidr.parse("::1/128"),
    )
    private val tailnetRanges = listOfNotNull(
        NetInfoCidr.parse("100.64.0.0/10"),
        NetInfoCidr.parse("fd7a:115c:a1e0::/48"),
    )

    fun isLinkLocal(a: String): Boolean = inAnyRange(a, linkLocalRanges)

    fun isLoopback(a: String): Boolean = inAnyRange(a, loopbackRanges)

    fun isTailnet(a: String): Boolean = inAnyRange(a, tailnetRanges)

    fun isVpn(iface: NetInfoRawIface, pools: List<NetInfoCidr>): Boolean {
        if (iface.addrs.any { address -> pools.any { it.contains(address.address) } }) return true
        if (!WorkProfileVpn.isTunnelName(iface.name)) return false
        return !iface.addrs.all { isTailnet(it.address) }
    }

    fun snapshot(raw: List<NetInfoRawIface>, pools: List<NetInfoCidr>): NetInfoSnapshot {
        val ifaces = raw.mapNotNull { rawIface ->
            val usable = rawIface.addrs.filterNot {
                isLinkLocal(it.address) || isLoopback(it.address)
            }
            val addresses = usable.mapNotNull { rawAddress ->
                val parsed = NetInfoCidr.parse(rawAddress.address) ?: return@mapNotNull null
                format(parsed.copy(prefixLength = rawAddress.prefixLength))
            }.distinct()
            if (addresses.isEmpty()) return@mapNotNull null
            NetInfoIface(
                name = rawIface.name,
                addrs = addresses,
                isVpn = isVpn(NetInfoRawIface(rawIface.name, usable), pools),
            )
        }
        return NetInfoSnapshot(ifaces)
    }

    private fun inAnyRange(address: String, ranges: List<NetInfoCidr>): Boolean {
        val parsed = NetInfoCidr.parse(address) ?: return false
        return ranges.any { it.contains(parsed) }
    }

    internal fun format(cidr: NetInfoCidr): String {
        val address = if (cidr.v6) formatIpv6(cidr.bytes) else cidr.bytes.joinToString(".")
        return "$address/${cidr.prefixLength}"
    }

    private fun formatIpv6(bytes: List<Int>): String {
        val words = bytes.chunked(2).map { (high, low) -> (high shl 8) or low }
        var bestStart = -1
        var bestLength = 0
        var index = 0
        while (index < words.size) {
            if (words[index] != 0) {
                index += 1
                continue
            }
            var end = index
            while (end < words.size && words[end] == 0) end += 1
            if (end - index > bestLength) {
                bestStart = index
                bestLength = end - index
            }
            index = end
        }

        val rendered = words.map { it.toString(16) }
        if (bestLength < 2) return rendered.joinToString(":")
        val left = rendered.take(bestStart).joinToString(":")
        val right = rendered.drop(bestStart + bestLength).joinToString(":")
        return when {
            left.isEmpty() && right.isEmpty() -> "::"
            left.isEmpty() -> "::$right"
            right.isEmpty() -> "$left::"
            else -> "$left::$right"
        }
    }
}
