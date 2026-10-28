package ai.gxlself.remotescreen.logic.session

const val KEYFRAME_DECODER_RESET = 2
const val KEYFRAME_BACKLOG = 4

data class FrameMeta(
    val frameId: Int,
    val epoch: Int,
    val keyframe: Boolean,
    val hasParameterSets: Boolean,
)

sealed interface GateAction {
    data class Drop(val frameId: Int) : GateAction

    data class RequestKeyframe(val reason: Int) : GateAction
}

class SessionFrameGate<T>(
    val maxBacklog: Int = 2,
    val rerequestAfterMs: Long = 500,
    val minRequestIntervalMs: Long = 300,
) {
    private val queue = ArrayDeque<Pair<FrameMeta, T>>()
    private var decoderIsReady = true
    private var waitStartedAtMs: Long? = null
    private var requestReason: Int? = null
    private var requestPending = false
    private var lastRequestAtMs: Long? = null
    private var lastRequestInWaitAtMs: Long? = null
    private var droppedCount = 0
    private var requestCount = 0

    init {
        require(maxBacklog >= 0) { "maxBacklog must not be negative" }
        require(rerequestAfterMs >= 0) { "rerequestAfterMs must not be negative" }
        require(minRequestIntervalMs >= 0) { "minRequestIntervalMs must not be negative" }
    }

    val pending: Int
        get() = queue.size

    val waitingForKeyframe: Boolean
        get() = waitStartedAtMs != null

    val dropped: Int
        get() = droppedCount

    val keyframeRequests: Int
        get() = requestCount

    fun push(meta: FrameMeta, payload: T, nowMs: Long): List<GateAction> {
        if (waitingForKeyframe) {
            if (!meta.keyframe || !meta.hasParameterSets) {
                return listOf(drop(meta.frameId))
            }
            leaveWaiting()
        }

        if (!decoderIsReady && !meta.keyframe) {
            val actions = dropQueued()
            actions += drop(meta.frameId)
            enterWaiting(KEYFRAME_BACKLOG, nowMs)
            maybeRequest(nowMs)?.let(actions::add)
            return actions
        }

        queue.addLast(meta to payload)
        if (queue.size <= maxBacklog) return emptyList()

        if (meta.keyframe && meta.hasParameterSets) {
            val actions = mutableListOf<GateAction>()
            while (queue.size > 1) {
                actions += drop(queue.removeFirst().first.frameId)
            }
            return actions
        }

        val actions = dropQueued()
        enterWaiting(KEYFRAME_BACKLOG, nowMs)
        maybeRequest(nowMs)?.let(actions::add)
        return actions
    }

    fun next(): Pair<FrameMeta, T>? = queue.removeFirstOrNull()

    fun decoderReady(ready: Boolean) {
        decoderIsReady = ready
    }

    fun decodeFailed(nowMs: Long): List<GateAction> {
        val actions = dropQueued()
        enterWaiting(KEYFRAME_DECODER_RESET, nowMs)
        maybeRequest(nowMs)?.let(actions::add)
        return actions
    }

    fun tick(nowMs: Long): List<GateAction> {
        val waitStarted = waitStartedAtMs ?: return emptyList()
        if (requestReason == null) {
            if (elapsed(nowMs, waitStarted) < rerequestAfterMs) return emptyList()
            requestReason = KEYFRAME_BACKLOG
            requestPending = true
        } else if (!requestPending) {
            val lastInWait = lastRequestInWaitAtMs ?: return emptyList()
            if (elapsed(nowMs, lastInWait) < rerequestAfterMs) return emptyList()
            requestPending = true
        }
        return listOfNotNull(maybeRequest(nowMs))
    }

    fun reset(waitForKeyframe: Boolean, nowMs: Long) {
        dropQueued()
        if (waitForKeyframe) {
            waitStartedAtMs = nowMs
            requestReason = null
            requestPending = false
            lastRequestInWaitAtMs = null
        } else {
            leaveWaiting()
        }
    }

    fun resetCounters() {
        droppedCount = 0
        requestCount = 0
    }

    private fun enterWaiting(reason: Int, nowMs: Long) {
        waitStartedAtMs = nowMs
        requestReason = reason
        requestPending = true
        lastRequestInWaitAtMs = null
    }

    private fun leaveWaiting() {
        waitStartedAtMs = null
        requestReason = null
        requestPending = false
        lastRequestInWaitAtMs = null
    }

    private fun maybeRequest(nowMs: Long): GateAction.RequestKeyframe? {
        val reason = requestReason ?: return null
        val lastRequest = lastRequestAtMs
        if (lastRequest != null && elapsed(nowMs, lastRequest) < minRequestIntervalMs) return null
        lastRequestAtMs = nowMs
        lastRequestInWaitAtMs = nowMs
        requestPending = false
        requestCount += 1
        return GateAction.RequestKeyframe(reason)
    }

    private fun dropQueued(): MutableList<GateAction> {
        val actions = mutableListOf<GateAction>()
        while (queue.isNotEmpty()) {
            actions += drop(queue.removeFirst().first.frameId)
        }
        return actions
    }

    private fun drop(frameId: Int): GateAction.Drop {
        droppedCount += 1
        return GateAction.Drop(frameId)
    }

    private fun elapsed(nowMs: Long, sinceMs: Long): Long =
        if (nowMs >= sinceMs) nowMs - sinceMs else Long.MIN_VALUE
}
