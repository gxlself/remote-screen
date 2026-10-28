package ai.gxlself.remotescreen.logic.common

/** Formats the marker lines consumed by the Remote Screen harness. */
object MarkerLine {
  private val tagPattern = Regex("[A-Z0-9-]{1,32}")
  private const val hex = "0123456789abcdef"

  fun format(tag: String, fields: Map<String, Any?>): String {
    if (!tagPattern.matches(tag)) {
      return "RS-INVALID ${encodeObject(mapOf("tag" to tag.take(32)))}"
    }

    return "RS-$tag ${encodeObject(fields)}"
  }

  private fun encodeObject(fields: Map<String, Any?>): String = buildString {
    append('{')
    fields.keys.sorted().forEachIndexed { index, key ->
      if (index > 0) append(',')
      append(encodeString(key))
      append(':')
      append(encodeValue(fields[key]))
    }
    append('}')
  }

  private fun encodeValue(value: Any?): String = when (value) {
    null -> "null"
    is String -> encodeString(value)
    is Boolean -> value.toString()
    is Int -> value.toString()
    is Long -> value.toString()
    is Double -> if (value.isFinite()) value.toString() else "null"
    is Float -> if (value.isFinite()) value.toString() else "null"
    is List<*> -> buildString {
      append('[')
      value.forEachIndexed { index, item ->
        if (index > 0) append(',')
        append(encodeValue(item))
      }
      append(']')
    }
    else -> encodeString("<unsupported>")
  }

  private fun encodeString(value: String): String = buildString {
    append('"')
    value.forEach { character ->
      when (character) {
        '"' -> append("\\\"")
        '\\' -> append("\\\\")
        '\n' -> append("\\n")
        '\r' -> append("\\r")
        '\t' -> append("\\t")
        else -> {
          if (character.code < 0x20) {
            append("\\u00")
            append(hex[(character.code ushr 4) and 0x0f])
            append(hex[character.code and 0x0f])
          } else {
            append(character)
          }
        }
      }
    }
    append('"')
  }
}
