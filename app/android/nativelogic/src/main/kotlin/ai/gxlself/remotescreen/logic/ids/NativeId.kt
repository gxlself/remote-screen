package ai.gxlself.remotescreen.logic.ids

/** Decimal u64 identifiers cross JNI as the same 64 bits in a signed Long. */
object NativeId {
    fun parse(value: String): Long? {
        if (value.isEmpty() || value.length > 20 || value.any { it !in '0'..'9' }) return null
        return try {
            java.lang.Long.parseUnsignedLong(value)
        } catch (_: NumberFormatException) {
            null
        }
    }

    fun format(id: Long): String = java.lang.Long.toUnsignedString(id)
}
