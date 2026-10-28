package ai.gxlself.remotescreen.logic.lifecycle

interface KeyValueBool {
  fun get(key: String): Boolean

  fun set(key: String, value: Boolean)
}

/** Marks a process that was last observed in the background. */
class ExitMarker(
  private val storage: KeyValueBool,
  private val key: String = DEFAULT_KEY,
) {
  fun start(): Boolean {
    val previous = storage.get(key)
    storage.set(key, false)
    return previous
  }

  fun markBackground() {
    storage.set(key, true)
  }

  fun markForeground() {
    storage.set(key, false)
  }

  companion object {
    const val DEFAULT_KEY = "rs.lifecycle.inBackground.v1"
    const val KEY = DEFAULT_KEY
  }
}

/** In-memory marker storage for JVM tests and previews. */
class InMemoryKeyValueBool : KeyValueBool {
  private val values = mutableMapOf<String, Boolean>()

  override fun get(key: String): Boolean = values[key] ?: false

  override fun set(key: String, value: Boolean) {
    values[key] = value
  }
}

typealias MemoryKeyValueBool = InMemoryKeyValueBool
