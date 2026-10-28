package ai.gxlself.remotescreen.logic.keyboard

object TextSplitter {
    /**
     * Packs already-segmented grapheme clusters into UTF-8 frames. An oversize cluster is
     * omitted because it cannot be represented by the text contract without splitting it.
     */
    fun split(clusters: List<String>, maxBytes: Int = 4096): List<String> {
        if (maxBytes <= 0) return emptyList()

        val parts = mutableListOf<String>()
        val part = StringBuilder()
        var bytes = 0
        for (cluster in clusters) {
            val count = cluster.toByteArray(Charsets.UTF_8).size
            if (count > maxBytes) {
                if (part.isNotEmpty()) parts += part.toString()
                part.clear()
                bytes = 0
                continue
            }
            if (bytes + count > maxBytes) {
                if (part.isNotEmpty()) parts += part.toString()
                part.clear()
                bytes = 0
            }
            part.append(cluster)
            bytes += count
        }
        if (part.isNotEmpty()) parts += part.toString()
        return parts
    }
}
