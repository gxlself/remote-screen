package ai.gxlself.remotescreen.logic.androidsession

import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized

@RunWith(Parameterized::class)
class ForegroundPlanTest(
    private val label: String,
    private val sessions: List<SessionRef>,
    private val sdkInt: Int,
    private val expected: ForegroundPlan,
) {
    @Test
    fun planMatchesTable() {
        assertEquals(label, expected, foregroundPlan(sessions, sdkInt))
    }

    companion object {
        @JvmStatic
        @Parameterized.Parameters(name = "{0}")
        fun cases(): Collection<Array<Any>> = listOf(
            arrayOf("empty list", emptyList<SessionRef>(), 34, ForegroundPlan.Stop),
            arrayOf(
                "one session",
                listOf(SessionRef("only", 10.0)),
                34,
                ForegroundPlan.Start("only", specialUseType = true),
            ),
            arrayOf(
                "earliest started session",
                listOf(
                    SessionRef("middle", 20.0),
                    SessionRef("late", 30.0),
                    SessionRef("early", 10.0),
                ),
                33,
                ForegroundPlan.Start("early", specialUseType = false),
            ),
            arrayOf(
                "session id breaks started-at tie",
                listOf(
                    SessionRef("zulu", 10.0),
                    SessionRef("alpha", 10.0),
                    SessionRef("mike", 20.0),
                ),
                33,
                ForegroundPlan.Start("alpha", specialUseType = false),
            ),
            arrayOf(
                "sdk 33",
                listOf(SessionRef("only", 10.0)),
                33,
                ForegroundPlan.Start("only", specialUseType = false),
            ),
            arrayOf(
                "sdk 34",
                listOf(SessionRef("only", 10.0)),
                34,
                ForegroundPlan.Start("only", specialUseType = true),
            ),
            arrayOf(
                "sdk 35",
                listOf(SessionRef("only", 10.0)),
                35,
                ForegroundPlan.Start("only", specialUseType = true),
            ),
        )
    }
}
