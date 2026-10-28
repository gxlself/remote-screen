package ai.gxlself.remotescreen.logic.androidsession

data class SessionRef(
    val sessionId: String,
    val startedAtMs: Double,
)

sealed interface ForegroundPlan {
    object Stop : ForegroundPlan

    data class Start(
        val primarySessionId: String,
        val specialUseType: Boolean,
    ) : ForegroundPlan
}

fun foregroundPlan(
    sessions: List<SessionRef>,
    sdkInt: Int,
    upsideDownCake: Int = 34,
): ForegroundPlan {
    if (sessions.isEmpty()) return ForegroundPlan.Stop
    val primary = sessions.minWithOrNull(
        compareBy<SessionRef> { it.startedAtMs }.thenBy { it.sessionId },
    ) ?: return ForegroundPlan.Stop
    return ForegroundPlan.Start(
        primarySessionId = primary.sessionId,
        specialUseType = sdkInt >= upsideDownCake,
    )
}
