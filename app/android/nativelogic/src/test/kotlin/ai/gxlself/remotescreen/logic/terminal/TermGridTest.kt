package ai.gxlself.remotescreen.logic.terminal

import org.junit.Assert.assertEquals
import org.junit.Test

class TermGridTest {
    @Test
    fun computesRequestedReferenceGrid() {
        assertEquals(50 to 50, TermGrid.size(1080, 1920, 21.6f, 38))
    }

    @Test
    fun deductsPaddingFromAllFourEdges() {
        assertEquals(8 to 3, TermGrid.size(100, 50, 10f, 10, paddingPx = 7))
    }

    @Test
    fun zeroViewSizeStillHasOneCell() {
        assertEquals(1 to 1, TermGrid.size(0, 0, 20f, 30))
    }

    @Test
    fun invalidCellWidthReturnsOneByOne() {
        assertEquals(1 to 1, TermGrid.size(1080, 1920, Float.NaN, 38))
        assertEquals(1 to 1, TermGrid.size(1080, 1920, Float.POSITIVE_INFINITY, 38))
        assertEquals(1 to 1, TermGrid.size(1080, 1920, 0f, 38))
        assertEquals(1 to 1, TermGrid.size(1080, 1920, -1f, 38))
    }

    @Test
    fun invalidLineHeightReturnsOneByOne() {
        assertEquals(1 to 1, TermGrid.size(1080, 1920, 21.6f, 0))
        assertEquals(1 to 1, TermGrid.size(1080, 1920, 21.6f, -1))
    }

    @Test
    fun tinyCellsAreCapped() {
        assertEquals(500 to 300, TermGrid.size(Int.MAX_VALUE, Int.MAX_VALUE, 0.001f, 1))
    }

    @Test
    fun excessiveAndNegativePaddingAreSafe() {
        assertEquals(1 to 1, TermGrid.size(100, 100, 10f, 10, Int.MAX_VALUE))
        assertEquals(10 to 10, TermGrid.size(100, 100, 10f, 10, -5))
    }
}
