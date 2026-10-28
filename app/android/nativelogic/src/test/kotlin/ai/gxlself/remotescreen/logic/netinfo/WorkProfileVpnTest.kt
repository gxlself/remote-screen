package ai.gxlself.remotescreen.logic.netinfo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkProfileVpnTest {
    @Test
    fun stripZoneHandlesAddressesWithAndWithoutZone() {
        assertEquals("fe80::1", WorkProfileVpn.stripZone("fe80::1%wlan0"))
        assertEquals("2001:db8::1", WorkProfileVpn.stripZone("2001:db8::1"))
    }

    @Test
    fun marksOtherTunnelButNotAppsDefaultVpn() {
        val marked = WorkProfileVpn.mark(snapshot(vpn("tun1"), vpn("tun0")), appVpnIface = "tun0")
        assertTrue(marked.named("tun1").workProfile)
        assertFalse(marked.named("tun0").workProfile)
    }

    @Test
    fun nullAppVpnMarksEveryVpnTunnel() {
        val marked = WorkProfileVpn.mark(snapshot(vpn("tun1"), vpn("wg0")), appVpnIface = null)
        assertTrue(marked.ifaces.all { it.workProfile })
    }

    @Test
    fun poolClassifiedPhysicalInterfaceIsNotMarked() {
        val marked = WorkProfileVpn.mark(snapshot(vpn("wlan0")), appVpnIface = null)
        assertFalse(marked.ifaces.single().workProfile)
    }

    @Test
    fun tailnetOnlyTunnelThatIsNotVpnIsNotMarked() {
        val marked = WorkProfileVpn.mark(
            snapshot(NetInfoIface("tun0", listOf("100.64.0.7/32"), isVpn = false)),
            appVpnIface = null,
        )
        assertFalse(marked.ifaces.single().workProfile)
    }

    @Test
    fun markingPreservesPathAndSortedOrdering() {
        val path = NetInfoPath("satisfied", listOf("wifi", "other"), expensive = false, constrained = true)
        val marked = WorkProfileVpn.mark(NetInfoSnapshot(listOf(vpn("wg1"), vpn("tun0")), path), "tun0")
        assertEquals(path, marked.path)
        assertEquals(listOf("tun0", "wg1"), marked.ifaces.map { it.name })
    }

    @Test
    fun tunnelNameMatchingUsesDocumentedCaseSensitivePrefixes() {
        for (name in listOf("utun0", "ipsec0", "ppp0", "tun0", "tap0", "wg0")) {
            assertTrue(name, WorkProfileVpn.isTunnelName(name))
        }
        assertFalse(WorkProfileVpn.isTunnelName("Wg0"))
        assertFalse(WorkProfileVpn.isTunnelName("wlan0"))
    }

    private fun snapshot(vararg ifaces: NetInfoIface) = NetInfoSnapshot(ifaces.toList())

    private fun vpn(name: String) = NetInfoIface(name, listOf("10.99.4.27/32"), isVpn = true)

    private fun NetInfoSnapshot.named(name: String) = ifaces.first { it.name == name }
}
