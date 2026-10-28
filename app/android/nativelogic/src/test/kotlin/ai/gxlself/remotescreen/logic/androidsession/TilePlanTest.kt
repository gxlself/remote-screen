package ai.gxlself.remotescreen.logic.androidsession

import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized

@RunWith(Parameterized::class)
class TilePlanTest(
    private val label: String,
    private val running: Boolean,
    private val targetHostId: String,
    private val targetLabel: String,
    private val expectedView: TileView,
    private val expectedClick: TileClick,
) {
    @Test
    fun viewAndClickMatchTable() {
        assertEquals(label, expectedView, tileView(running, targetHostId, targetLabel))
        assertEquals(label, expectedClick, tileClick(running, targetHostId))
    }

    companion object {
        @JvmStatic
        @Parameterized.Parameters(name = "{0}")
        fun cases(): Collection<Array<Any>> = listOf(
            arrayOf(
                "running with target",
                true, "host-a", "Studio",
                TileView(true, TileSubtitle.ReturnToSession, "active"),
                TileClick.Resume,
            ),
            arrayOf(
                "running without target",
                true, "", "",
                TileView(true, TileSubtitle.ReturnToSession, "active"),
                TileClick.Resume,
            ),
            arrayOf(
                "idle with target",
                false, "host-a", "Studio",
                TileView(false, TileSubtitle.Target("Studio"), "idle"),
                TileClick.Connect("host-a"),
            ),
            arrayOf(
                "idle without target",
                false, "", "",
                TileView(false, TileSubtitle.OpenApp, "empty"),
                TileClick.Open,
            ),
        )
    }
}
