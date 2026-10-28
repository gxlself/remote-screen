package ai.gxlself.remotescreen.logic.netinfo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NetInfoCidrTest {
    @Test
    fun cidrParsingAndContainsMatchesIos() {
        val v4 = NetInfoCidr.parse("10.99.0.0/16")
        assertTrue(v4?.contains("10.99.255.1") == true)
        assertFalse(v4?.contains("10.98.0.1") == true)
        assertTrue(NetInfoCidr.parse("fd7a:115c:a1e0::/48")?.contains("fd7a:115c:a1e0:ab12::1") == true)
        assertNull(NetInfoCidr.parse("10.99.0.0/nope"))
        assertNull(NetInfoCidr.parse("not-an-address"))
    }

    @Test
    fun hostAddressesGetFullLengthPrefixesAndOuterWhitespaceIsTrimmed() {
        assertEquals(32, NetInfoCidr.parse("  192.168.31.7\n")?.prefixLength)
        assertEquals(128, NetInfoCidr.parse("\t2001:DB8::1 ")?.prefixLength)
    }

    @Test
    fun ipv4ParserRejectsMalformedAndLeadingZeroAddresses() {
        for (value in listOf("256.1.1.1", "1.2.3", "01.2.3.4", "1..2.3", "+1.2.3.4", "1.2.3.4.5")) {
            assertNull(value, NetInfoCidr.parse(value))
        }
    }

    @Test
    fun parserRejectsBadIpv6Forms() {
        for (value in listOf("1::2::3", "12345::", "1:2:3:4:5:6:7", "1:2:3:4:5:6:7:8:9", "1.2.3.4::")) {
            assertNull(value, NetInfoCidr.parse(value))
        }
    }

    @Test
    fun parserRejectsBadPrefixesAndEmptyInput() {
        for (value in listOf("10.0.0.0/33", "::/129", "", "  ", "10.0.0.1/", "/24", "10.0.0.1/24/7")) {
            assertNull(value, NetInfoCidr.parse(value))
        }
    }

    @Test
    fun parsesIpv6WithTrailingEmbeddedIpv4() {
        val parsed = NetInfoCidr.parse("::ffff:1.2.3.4")
        assertNotNull(parsed)
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 1, 2, 3, 4), parsed?.bytes)
    }

    @Test
    fun cidrFamiliesNeverContainEachOther() {
        assertFalse(NetInfoCidr.parse("0.0.0.0/0")!!.contains("::1"))
        assertFalse(NetInfoCidr.parse("::/0")!!.contains("127.0.0.1"))
    }

    @Test
    fun containsHandlesPartialBytePrefixes() {
        val tailnet = NetInfoCidr.parse("100.64.0.0/10")!!
        assertTrue(tailnet.contains("100.127.255.255"))
        assertFalse(tailnet.contains("100.128.0.0"))
    }

    @Test
    fun parserRejectsZonesUntilCallerStripsThem() {
        assertNull(NetInfoCidr.parse("fe80::1%wlan0"))
        assertNotNull(NetInfoCidr.parse(WorkProfileVpn.stripZone("fe80::1%wlan0")))
    }
}
