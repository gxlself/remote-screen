package ai.gxlself.remotescreen.logic.netinfo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NetInfoDebouncerAndPathTest {
    @Test
    fun debouncerUsesTrailingEdgeLikeIos() {
        val debouncer = NetInfoDebouncer(delayUs = 500)
        var fires = 0
        debouncer.update(atUs = 0)
        debouncer.update(atUs = 100)
        debouncer.update(atUs = 300)
        debouncer.fireIfDue(atUs = 799) { fires += 1 }
        assertEquals(0, fires)
        debouncer.fireIfDue(atUs = 800) { fires += 1 }
        assertEquals(1, fires)
        debouncer.fireIfDue(atUs = 900) { fires += 1 }
        assertEquals(1, fires)
    }

    @Test
    fun debouncerReportsDueAndClearsBeforeAction() {
        val debouncer = NetInfoDebouncer(delayUs = 20)
        debouncer.update(10)
        assertFalse(debouncer.isDue(29))
        assertTrue(debouncer.isDue(30))
        debouncer.fireIfDue(30) { assertFalse(debouncer.isDue(30)) }
    }

    @Test
    fun pathMapsWifi() {
        val path = NetPathMapper.path(true, true, setOf(1), notMetered = true, dataSaver = false)
        assertEquals("satisfied", path.status)
        assertEquals(listOf("wifi"), path.interfaceTypes)
        assertFalse(path.expensive)
        assertFalse(path.constrained)
    }

    @Test
    fun pathMapsWifiAndVpnInFixedOrder() {
        val path = NetPathMapper.path(true, true, setOf(4, 1), notMetered = true, dataSaver = false)
        assertEquals(listOf("wifi", "other"), path.interfaceTypes)
    }

    @Test
    fun pathMapsMeteredCellularAsExpensive() {
        val path = NetPathMapper.path(true, true, setOf(0), notMetered = false, dataSaver = false)
        assertEquals(listOf("cellular"), path.interfaceTypes)
        assertTrue(path.expensive)
    }

    @Test
    fun pathWithoutNetworkIsUnsatisfiedAndNotExpensive() {
        val path = NetPathMapper.path(false, false, emptySet(), notMetered = false, dataSaver = false)
        assertEquals(NetInfoPath.UNKNOWN, path)
    }

    @Test
    fun pathWithoutInternetCapabilityIsUnsatisfied() {
        assertEquals(
            "unsatisfied",
            NetPathMapper.path(true, false, setOf(3), notMetered = true, dataSaver = false).status,
        )
    }

    @Test
    fun pathMapsDataSaverToConstrained() {
        assertTrue(NetPathMapper.path(true, true, setOf(1), notMetered = true, dataSaver = true).constrained)
    }

    @Test
    fun pathDeduplicatesWiredAndOtherCategories() {
        val path = NetPathMapper.path(true, true, setOf(3, 8, 2, 4, 99), notMetered = true, dataSaver = false)
        assertEquals(listOf("wiredEthernet", "other"), path.interfaceTypes)
    }
}
