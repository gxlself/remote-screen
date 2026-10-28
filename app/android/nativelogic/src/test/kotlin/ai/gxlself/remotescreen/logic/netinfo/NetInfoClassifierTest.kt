package ai.gxlself.remotescreen.logic.netinfo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NetInfoClassifierTest {
    @Test
    fun classifierMatchesIosA16HVectors() {
        val pools = listOf(NetInfoCidr.parse("10.99.0.0/16")!!)
        val status = NetInfoClassifier.snapshot(
            listOf(
                iface("en0", "192.168.31.20/24", "fe80::1/64"),
                iface("lo0", "127.0.0.1/8"),
                iface("utun4", "100.64.0.21/32"),
                iface("utun5", "10.99.4.27/32"),
                iface("ipsec0", "10.30.8.9/32"),
                iface("en2", "10.99.4.27/24"),
                iface("utun0", "fe80::2/64"),
            ),
            pools,
        )

        assertEquals(listOf("en0", "en2", "ipsec0", "utun4", "utun5"), status.ifaces.map { it.name })
        assertEquals(listOf("192.168.31.20/24"), status.named("en0").addrs)
        assertFalse(status.named("en0").isVpn)
        assertFalse(status.named("utun4").isVpn)
        assertTrue(status.named("utun5").isVpn)
        assertTrue(status.named("ipsec0").isVpn)
        assertTrue(status.named("en2").isVpn)
        assertTrue(NetInfoClassifier.snapshot(listOf(iface("utun5", "10.99.4.27/32")), emptyList()).ifaces[0].isVpn)
    }

    @Test
    fun snapshotEqualityAndOrderingMatchesIos() {
        val raw = listOf(iface("z", "10.30.1.2/16"), iface("a", "192.168.31.20/24"))
        val first = NetInfoClassifier.snapshot(raw, emptyList())
        val second = NetInfoClassifier.snapshot(raw, emptyList())
        assertEquals(first, second)
        assertNotEquals(first, NetInfoClassifier.snapshot(raw + iface("a", "10.30.1.2/16"), emptyList()))
        assertEquals(listOf("a", "z"), first.ifaces.map { it.name })
    }

    @Test
    fun snapshotConstructorAlwaysSortsByName() {
        val snapshot = NetInfoSnapshot(
            listOf(
                NetInfoIface("z", listOf("10.0.0.1/8"), false),
                NetInfoIface("a", listOf("10.0.0.2/8"), false),
            ),
        )
        assertEquals(listOf("a", "z"), snapshot.ifaces.map { it.name })
    }

    @Test
    fun snapshotFiltersLoopbackLinkLocalAndEmptyInterfaces() {
        val snapshot = NetInfoClassifier.snapshot(
            listOf(
                iface("lo", "127.0.0.2/8", "::1/128"),
                iface("links", "169.254.2.3/16", "febf::1/64"),
                iface("good", "169.253.255.255/24", "fec0::1/64"),
            ),
            emptyList(),
        )
        assertEquals(listOf("good"), snapshot.ifaces.map { it.name })
    }

    @Test
    fun snapshotDeduplicatesCanonicalAddressesPreservingFirstOrder() {
        val snapshot = NetInfoClassifier.snapshot(
            listOf(iface("en0", "2001:0DB8:0:0:0:0:2:1/64", "2001:db8::2:1/64", "192.168.31.7/24")),
            emptyList(),
        )
        assertEquals(listOf("2001:db8::2:1/64", "192.168.31.7/24"), snapshot.ifaces.single().addrs)
    }

    @Test
    fun ipv6FormattingCompressesLongestRun() {
        assertFormatted("2001:db8:0:0:0:0:2:1/64", "2001:db8::2:1/64")
        assertFormatted("0:0:0:0:0:0:0:1/128", "::1/128")
    }

    @Test
    fun ipv6FormattingUsesFirstRunWhenLengthsTie() {
        assertFormatted("fe80:0:0:0:1:0:0:0/64", "fe80::1:0:0:0/64")
    }

    @Test
    fun ipv6FormattingDoesNotCompressOneZeroWord() {
        assertFormatted("2001:db8:0:1:2:3:4:5/64", "2001:db8:0:1:2:3:4:5/64")
    }

    @Test
    fun tunnelFallbackExcludesTailnetOnlyAndEmptyInterfaces() {
        for (name in listOf("utun3", "ipsec0", "ppp0", "tun0", "tap0", "wg0")) {
            assertTrue(NetInfoClassifier.isVpn(iface(name, "10.30.4.27/32"), emptyList()))
            assertFalse(NetInfoClassifier.isVpn(iface(name, "100.64.0.12/32", "fd7a:115c:a1e0::12/128"), emptyList()))
            assertFalse(NetInfoClassifier.isVpn(NetInfoRawIface(name, emptyList()), emptyList()))
        }
    }

    @Test
    fun registeredPoolOverridesTailnetTunnelExclusion() {
        val pool = NetInfoCidr.parse("fd7a:115c:a1e0::/48")!!
        assertTrue(NetInfoClassifier.isVpn(iface("utun3", "fd7a:115c:a1e0::12/128"), listOf(pool)))
    }

    private fun assertFormatted(input: String, expected: String) {
        assertEquals(expected, NetInfoClassifier.format(NetInfoCidr.parse(input)!!))
    }

    private fun NetInfoSnapshot.named(name: String): NetInfoIface = ifaces.first { it.name == name }
}

private fun iface(name: String, vararg addresses: String): NetInfoRawIface =
    NetInfoRawIface(
        name,
        addresses.map { value ->
            val pieces = value.split('/', limit = 2)
            NetInfoRawAddress(pieces[0], pieces.getOrNull(1)?.toInt() ?: if (':' in pieces[0]) 128 else 32)
        },
    )
