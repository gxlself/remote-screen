package ai.gxlself.remotescreen.logic.files

/** Turns provider display names into bounded, unique path components. */
object FileNameRules {
  const val DEFAULT_NAME = "file"
  const val MAX_NAME_BYTES = 255

  fun sanitize(name: String?): String {
    val filtered = name.orEmpty().filter { character ->
      character != '/' && character != '\\' && !character.isISOControl()
    }
    val safe = filtered.takeUnless { it.isBlank() || it == "." || it == ".." } ?: DEFAULT_NAME
    val (stem, extension) = splitExtension(safe)
    return fit(stem, extension)
  }

  fun unique(name: String?, taken: Set<String>): String {
    val base = sanitize(name)
    if (base !in taken) return base

    val (stem, extension) = splitExtension(base)
    var index = 2L
    while (true) {
      val candidate = fit(stem, extension, " ($index)")
      if (candidate !in taken) return candidate
      index += 1
    }
  }

  private fun splitExtension(name: String): Pair<String, String> {
    val dot = name.lastIndexOf('.')
    return if (dot <= 0 || dot == name.lastIndex) {
      name to ""
    } else {
      name.substring(0, dot) to name.substring(dot)
    }
  }

  private fun fit(stem: String, extension: String, collision: String = ""): String {
    // Reserve the counter before truncating so long names cannot lose it and loop forever.
    val budget = MAX_NAME_BYTES - collision.length
    val suffix = utf8Prefix(extension, budget - 1)
    val stemBudget = budget - suffix.toByteArray(Charsets.UTF_8).size
    val prefix = utf8Prefix(stem, stemBudget).ifEmpty { DEFAULT_NAME.take(stemBudget) }
    return prefix + collision + suffix
  }

  private fun utf8Prefix(value: String, maxBytes: Int): String {
    var end = 0
    var bytes = 0
    while (end < value.length) {
      val point = value.codePointAt(end)
      val width = when {
        point <= 0x7f -> 1
        point <= 0x7ff -> 2
        point <= 0xffff -> 3
        else -> 4
      }
      if (bytes + width > maxBytes) break
      bytes += width
      end += Character.charCount(point)
    }
    return value.substring(0, end)
  }
}
