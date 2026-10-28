package ai.gxlself.remotescreen.logic.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized

@RunWith(Parameterized::class)
class DecoderRebuildTest(
    private val label: String,
    private val hasCodec: Boolean,
    private val currentEpoch: Int?,
    private val frameEpoch: Int,
    private val currentCodec: Int,
    private val frameCodec: Int,
    private val keyframe: Boolean,
    private val setsChanged: Boolean?,
    private val expected: DecoderAction?,
) {
    @Test
    fun decisionMatchesTable() {
        if (expected == null) {
            val error = assertThrows(KeyframeRequiredException::class.java) {
                decoderAction(
                    hasCodec,
                    currentEpoch,
                    frameEpoch,
                    currentCodec,
                    frameCodec,
                    keyframe,
                    setsChanged,
                )
            }
            assertEquals(label, "keyframe with parameter sets required", error.message)
            return
        }
        assertEquals(
            label,
            expected,
            decoderAction(
                hasCodec,
                currentEpoch,
                frameEpoch,
                currentCodec,
                frameCodec,
                keyframe,
                setsChanged,
            ),
        )
    }

    companion object {
        @JvmStatic
        @Parameterized.Parameters(name = "{0}")
        fun cases(): Collection<Array<Any?>> = listOf(
            arrayOf<Any?>(
                "missing codec, keyframe, unchanged sets",
                false, null, 1, 0, 265, true, false, DecoderAction.Create,
            ),
            arrayOf<Any?>(
                "missing codec, non-keyframe",
                false, null, 1, 0, 265, false, false, null,
            ),
            arrayOf<Any?>(
                "missing codec, keyframe, no parameter sets",
                false, null, 1, 0, 265, true, null, null,
            ),
            arrayOf<Any?>(
                "unchanged non-keyframe without parameter sets",
                true, 7, 7, 265, 265, false, null, DecoderAction.Feed,
            ),
            arrayOf<Any?>(
                "keyframe with unchanged parameter sets",
                true, 7, 7, 265, 265, true, false, DecoderAction.Feed,
            ),
            arrayOf<Any?>(
                "changed epoch, keyframe, parameter sets",
                true, 6, 7, 265, 265, true, false, DecoderAction.Rebuild,
            ),
            arrayOf<Any?>(
                "changed codec 264 to 265, keyframe, parameter sets",
                true, 7, 7, 264, 265, true, false, DecoderAction.Rebuild,
            ),
            arrayOf<Any?>(
                "changed parameter sets, keyframe",
                true, 7, 7, 265, 265, true, true, DecoderAction.Rebuild,
            ),
            arrayOf<Any?>(
                "changed epoch, non-keyframe",
                true, 6, 7, 265, 265, false, false, null,
            ),
        )
    }
}
