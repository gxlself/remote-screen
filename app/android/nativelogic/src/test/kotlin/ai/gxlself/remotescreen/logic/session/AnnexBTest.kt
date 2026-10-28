package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AnnexBTest {
    @Test
    fun convertsSingleNal() {
        val bytes = byteArrayOf(0, 0, 0, 3, 5, 6, 7)
        assertTrue(AnnexB.avccToAnnexBInPlace(bytes))
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 5, 6, 7), bytes)
    }

    @Test
    fun convertsMultipleNals() {
        val bytes = byteArrayOf(0, 0, 0, 1, 5, 0, 0, 0, 2, 6, 7)
        assertTrue(AnnexB.avccToAnnexBInPlace(bytes))
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 5, 0, 0, 0, 1, 6, 7), bytes)
    }

    @Test
    fun rejectsNalLengthBeyondBuffer() {
        assertFalse(AnnexB.avccToAnnexBInPlace(byteArrayOf(0, 0, 0, 2, 5)))
    }

    @Test
    fun rejectsZeroLengthNal() {
        assertFalse(AnnexB.avccToAnnexBInPlace(byteArrayOf(0, 0, 0, 0)))
    }

    @Test
    fun rejectsTrailingPartialLength() {
        val bytes = byteArrayOf(0, 0, 0, 1, 5, 0, 0)
        assertFalse(AnnexB.avccToAnnexBInPlace(bytes))
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 5, 0, 0), bytes)
    }

    @Test
    fun acceptsEmptyAccessUnit() {
        assertTrue(AnnexB.avccToAnnexBInPlace(byteArrayOf()))
    }

    @Test
    fun validatesExplicitLength() {
        val bytes = byteArrayOf(0, 0, 0, 1, 5, 99)
        assertTrue(AnnexB.avccToAnnexBInPlace(bytes, len = 5))
        assertFalse(AnnexB.avccToAnnexBInPlace(bytes, len = 7))
        assertFalse(AnnexB.avccToAnnexBInPlace(bytes, len = -1))
    }

    @Test
    fun createsH264CsdBuffers() {
        val csd = AnnexB.csd(codec = 2, vps = null, sps = byteArrayOf(7), pps = byteArrayOf(8))
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 7), csd[0])
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 8), csd[1])
    }

    @Test
    fun createsCombinedHevcCsdBuffer() {
        val csd = AnnexB.csd(1, byteArrayOf(6), byteArrayOf(7), byteArrayOf(8))
        assertArrayEquals(
            byteArrayOf(0, 0, 0, 1, 6, 0, 0, 0, 1, 7, 0, 0, 0, 1, 8),
            csd.single(),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun hevcRequiresVps() {
        AnnexB.csd(1, null, byteArrayOf(7), byteArrayOf(8))
    }

    @Test
    fun comparesParameterSetsByContent() {
        val first = listOf(byteArrayOf(1, 2), byteArrayOf(3))
        val equalCopy = listOf(byteArrayOf(1, 2), byteArrayOf(3))
        assertTrue(AnnexB.sameParameterSets(first, equalCopy))
        assertFalse(AnnexB.sameParameterSets(first, listOf(byteArrayOf(1, 9), byteArrayOf(3))))
        assertFalse(AnnexB.sameParameterSets(first, listOf(byteArrayOf(1, 2))))
        assertFalse(AnnexB.sameParameterSets(null, equalCopy))
    }
}
