package ai.gxlself.remotescreen.logic.terminal

object TermPasteEncoder {
    fun encode(text: String, bracketed: Boolean): ByteArray {
        val body =
            text
                .replace("\u001b", "")
                .replace("\r\n", "\r")
                .replace("\n", "\r")
                .toByteArray(Charsets.UTF_8)
        if (!bracketed) return body
        return BRACKETED_START + body + BRACKETED_END
    }

    private val BRACKETED_START = "\u001b[200~".toByteArray(Charsets.UTF_8)
    private val BRACKETED_END = "\u001b[201~".toByteArray(Charsets.UTF_8)
}
