package ai.gxlself.remotescreen.logic.androidsession

sealed interface TileSubtitle {
    object ReturnToSession : TileSubtitle

    data class Target(val label: String) : TileSubtitle

    object OpenApp : TileSubtitle
}

data class TileView(
    val active: Boolean,
    val subtitle: TileSubtitle,
    val marker: String,
)

fun tileView(running: Boolean, targetHostId: String, targetLabel: String): TileView = when {
    running -> TileView(
        active = true,
        subtitle = TileSubtitle.ReturnToSession,
        marker = "active",
    )

    targetHostId.isNotEmpty() -> TileView(
        active = false,
        subtitle = TileSubtitle.Target(targetLabel),
        marker = "idle",
    )

    else -> TileView(
        active = false,
        subtitle = TileSubtitle.OpenApp,
        marker = "empty",
    )
}

sealed interface TileClick {
    object Resume : TileClick

    data class Connect(val hostId: String) : TileClick

    object Open : TileClick
}

fun tileClick(running: Boolean, targetHostId: String): TileClick = when {
    running -> TileClick.Resume
    targetHostId.isNotEmpty() -> TileClick.Connect(targetHostId)
    else -> TileClick.Open
}
