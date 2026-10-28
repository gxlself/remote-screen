package ai.gxlself.remotescreen.logic.netinfo

class NetInfoDebouncer(val delayUs: Long = 500_000) {
    private var deadlineUs: Long? = null

    fun update(atUs: Long) {
        deadlineUs = atUs + delayUs
    }

    fun fireIfDue(atUs: Long, action: () -> Unit) {
        if (!isDue(atUs)) return
        deadlineUs = null
        action()
    }

    fun isDue(atUs: Long): Boolean = deadlineUs?.let { atUs >= it } ?: false
}
