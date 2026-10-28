package ai.gxlself.remotescreen.rsnative.keyboard

import ai.gxlself.remotescreen.logic.keyboard.GraphemeSplitter
import android.icu.text.BreakIterator
import java.util.Locale

/** Android ICU implementation of the shared grapheme segmentation contract. */
class IcuGraphemeSplitter : GraphemeSplitter {
  override fun clusters(text: String): List<String> {
    if (text.isEmpty()) return emptyList()

    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    val result = ArrayList<String>()
    var start = iterator.first()
    var end = iterator.next()
    while (end != BreakIterator.DONE) {
      result += text.substring(start, end)
      start = end
      end = iterator.next()
    }
    return result
  }
}
