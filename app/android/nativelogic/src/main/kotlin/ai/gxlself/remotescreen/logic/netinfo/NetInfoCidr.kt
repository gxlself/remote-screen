package ai.gxlself.remotescreen.logic.netinfo

data class NetInfoCidr(
    val v6: Boolean,
    val bytes: List<Int>,
    val prefixLength: Int,
) {
    companion object {
        fun parse(value: String): NetInfoCidr? {
            val trimmed = value.trim()
            if (trimmed.isEmpty()) return null

            val slash = trimmed.indexOf('/')
            if (slash != trimmed.lastIndexOf('/')) return null
            val address = if (slash < 0) trimmed else trimmed.substring(0, slash)
            if (address.isEmpty()) return null

            val parsed = if (address.contains(':')) parseIpv6(address) else parseIpv4(address)
            parsed ?: return null
            val maximumPrefix = if (parsed.size == 16) 128 else 32
            val prefix = if (slash < 0) {
                maximumPrefix
            } else {
                parsePrefix(trimmed.substring(slash + 1)) ?: return null
            }
            if (prefix !in 0..maximumPrefix) return null
            return NetInfoCidr(v6 = parsed.size == 16, bytes = parsed, prefixLength = prefix)
        }

        private fun parsePrefix(value: String): Int? {
            if (value.isEmpty() || value.any { it !in '0'..'9' }) return null
            return value.toIntOrNull()
        }

        private fun parseIpv4(value: String): List<Int>? {
            val pieces = value.split('.', ignoreCase = false, limit = 5)
            if (pieces.size != 4) return null
            return pieces.map { piece ->
                if (piece.isEmpty() || piece.any { it !in '0'..'9' }) return null
                if (piece.length > 1 && piece[0] == '0') return null
                val octet = piece.toIntOrNull() ?: return null
                if (octet !in 0..255) return null
                octet
            }
        }

        private fun parseIpv6(value: String): List<Int>? {
            if ('%' in value) return null
            val compression = value.indexOf("::")
            if (compression >= 0 && value.indexOf("::", compression + 2) >= 0) return null

            val tokens = mutableListOf<String>()
            val hasCompression = compression >= 0
            if (hasCompression) {
                val left = value.substring(0, compression)
                val right = value.substring(compression + 2)
                parseIpv6Side(left)?.let(tokens::addAll) ?: return null
                parseIpv6Side(right)?.let(tokens::addAll) ?: return null
            } else {
                parseIpv6Side(value)?.let(tokens::addAll) ?: return null
            }

            val words = mutableListOf<Int>()
            tokens.forEachIndexed { index, token ->
                if ('.' in token) {
                    if (index != tokens.lastIndex) return null
                    if (!value.endsWith(token)) return null
                    val ipv4 = parseIpv4(token) ?: return null
                    words += (ipv4[0] shl 8) or ipv4[1]
                    words += (ipv4[2] shl 8) or ipv4[3]
                } else {
                    if (token.length !in 1..4 || token.any { !it.isAsciiHexDigit() }) return null
                    words += token.toIntOrNull(16) ?: return null
                }
            }

            if (hasCompression) {
                if (words.size >= 8) return null
                val zeroCount = 8 - words.size
                val leftTokenCount = parseIpv6Side(value.substring(0, compression))?.size ?: return null
                val leftWordCount = tokens.take(leftTokenCount).sumOf { if ('.' in it) 2 else 1 }
                repeat(zeroCount) { words.add(leftWordCount, 0) }
            } else if (words.size != 8) {
                return null
            }

            return words.flatMap { word -> listOf((word ushr 8) and 0xff, word and 0xff) }
        }

        private fun parseIpv6Side(value: String): List<String>? {
            if (value.isEmpty()) return emptyList()
            val pieces = value.split(':')
            return pieces.takeIf { parts -> parts.none(String::isEmpty) }
        }

        private fun Char.isAsciiHexDigit(): Boolean =
            this in '0'..'9' || this in 'a'..'f' || this in 'A'..'F'
    }

    fun contains(address: String): Boolean {
        val parsed = parse(address) ?: return false
        return contains(parsed)
    }

    fun contains(other: NetInfoCidr): Boolean {
        if (v6 != other.v6 || bytes.size != other.bytes.size) return false
        val wholeBytes = prefixLength / 8
        val remainingBits = prefixLength % 8
        if (bytes.take(wholeBytes) != other.bytes.take(wholeBytes)) return false
        if (remainingBits == 0) return true
        val mask = (0xff shl (8 - remainingBits)) and 0xff
        return bytes[wholeBytes] and mask == other.bytes[wholeBytes] and mask
    }
}
