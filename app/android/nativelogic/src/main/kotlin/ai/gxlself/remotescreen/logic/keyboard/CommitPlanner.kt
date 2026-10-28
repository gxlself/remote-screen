package ai.gxlself.remotescreen.logic.keyboard

object CommitPlanner {
    fun plan(
        committed: String,
        mode: InputMode,
        stickyMods: Int,
        splitter: GraphemeSplitter,
    ): List<KeyAction> {
        val actions = mutableListOf<KeyAction>()
        val clusters = splitter.clusters(committed)
        val pendingText = mutableListOf<String>()

        fun flushText() {
            actions += TextSplitter.split(pendingText).map(KeyAction::Text)
            pendingText.clear()
        }

        fun appendKey(usage: Int, mods: Int) {
            actions += KeyAction.Key(usage, down = true, mods)
            actions += KeyAction.Key(usage, down = false, mods)
        }

        var firstRemaining = 0
        if (stickyMods != 0 && clusters.isNotEmpty()) {
            HidMap.usage(clusters.first())?.let { (usage, needsShift) ->
                appendKey(usage, stickyMods or if (needsShift) LEFT_SHIFT else 0)
                actions += KeyAction.StickyConsumed(stickyMods)
                firstRemaining = 1
            }
        }

        for (index in firstRemaining until clusters.size) {
            val cluster = clusters[index]
            val mapped = if (mode == InputMode.KEYS) HidMap.usage(cluster) else null
            when {
                mapped != null -> {
                    flushText()
                    appendKey(mapped.first, if (mapped.second) LEFT_SHIFT else 0)
                }
                cluster == "\n" -> {
                    flushText()
                    appendKey(ENTER_USAGE, 0)
                }
                else -> pendingText += cluster
            }
        }
        flushText()
        return actions
    }

    private const val LEFT_SHIFT = 0x02
    private const val ENTER_USAGE = 0x28
}
