package ai.gxlself.remotescreen.logic.gestures

/** Runs a lifecycle cleanup action at most once. */
class CloseOnce(private val action: () -> Unit) {
    @Volatile
    var isClosed: Boolean = false
        private set

    fun close(): Boolean {
        synchronized(this) {
            if (isClosed) return false
            isClosed = true
        }
        action()
        return true
    }
}
