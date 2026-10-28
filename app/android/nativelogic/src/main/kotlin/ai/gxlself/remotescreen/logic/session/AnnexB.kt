package ai.gxlself.remotescreen.logic.session

object AnnexB {
    private val startCode = byteArrayOf(0, 0, 0, 1)

    fun avccToAnnexBInPlace(buf: ByteArray, len: Int = buf.size): Boolean {
        if (len < 0 || len > buf.size) return false
        var offset = 0
        while (offset < len) {
            if (len - offset < 4) return false
            val nalLength =
                ((buf[offset].toLong() and 0xffL) shl 24) or
                    ((buf[offset + 1].toLong() and 0xffL) shl 16) or
                    ((buf[offset + 2].toLong() and 0xffL) shl 8) or
                    (buf[offset + 3].toLong() and 0xffL)
            if (nalLength == 0L || nalLength > (len - offset - 4).toLong()) return false
            buf[offset] = 0
            buf[offset + 1] = 0
            buf[offset + 2] = 0
            buf[offset + 3] = 1
            offset += 4 + nalLength.toInt()
        }
        return true
    }

    fun csd(codec: Int, vps: ByteArray?, sps: ByteArray, pps: ByteArray): List<ByteArray> =
        when (codec) {
            1 -> {
                val hevcVps = requireNotNull(vps) { "HEVC requires a VPS" }
                require(hevcVps.isNotEmpty()) { "HEVC requires a VPS" }
                listOf(startCode + hevcVps + startCode + sps + startCode + pps)
            }
            2 -> listOf(startCode + sps, startCode + pps)
            else -> throw IllegalArgumentException("unsupported codec: $codec")
        }

    fun sameParameterSets(a: List<ByteArray>?, b: List<ByteArray>): Boolean {
        if (a == null || a.size != b.size) return false
        return a.indices.all { a[it].contentEquals(b[it]) }
    }
}
