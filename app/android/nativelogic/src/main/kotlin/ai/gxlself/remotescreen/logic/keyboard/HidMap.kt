package ai.gxlself.remotescreen.logic.keyboard

/** US ANSI character positions. Non-ASCII input stays on the text path. */
object HidMap {
    private val punctuation =
        mapOf(
            '-' to (0x2D to false),
            '_' to (0x2D to true),
            '=' to (0x2E to false),
            '+' to (0x2E to true),
            '[' to (0x2F to false),
            '{' to (0x2F to true),
            ']' to (0x30 to false),
            '}' to (0x30 to true),
            '\\' to (0x31 to false),
            '|' to (0x31 to true),
            ';' to (0x33 to false),
            ':' to (0x33 to true),
            '\'' to (0x34 to false),
            '"' to (0x34 to true),
            '`' to (0x35 to false),
            '~' to (0x35 to true),
            ',' to (0x36 to false),
            '<' to (0x36 to true),
            '.' to (0x37 to false),
            '>' to (0x37 to true),
            '/' to (0x38 to false),
            '?' to (0x38 to true),
            '!' to (0x1E to true),
            '@' to (0x1F to true),
            '#' to (0x20 to true),
            '$' to (0x21 to true),
            '%' to (0x22 to true),
            '^' to (0x23 to true),
            '&' to (0x24 to true),
            '*' to (0x25 to true),
            '(' to (0x26 to true),
            ')' to (0x27 to true),
        )

    fun usage(cluster: String): Pair<Int, Boolean>? {
        if (cluster.codePointCount(0, cluster.length) != 1) return null
        val codePoint = cluster.codePointAt(0)
        return when (codePoint) {
            in 'a'.code..'z'.code -> (codePoint - 'a'.code + 0x04) to false
            in 'A'.code..'Z'.code -> (codePoint - 'A'.code + 0x04) to true
            in '1'.code..'9'.code -> (codePoint - '1'.code + 0x1E) to false
            '0'.code -> 0x27 to false
            ' '.code -> 0x2C to false
            '\n'.code -> 0x28 to false
            '\t'.code -> 0x2B to false
            else -> if (codePoint <= Char.MAX_VALUE.code) punctuation[codePoint.toChar()] else null
        }
    }
}
