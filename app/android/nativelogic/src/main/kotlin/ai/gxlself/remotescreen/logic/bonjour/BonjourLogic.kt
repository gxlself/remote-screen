package ai.gxlself.remotescreen.logic.bonjour

/** Data sent to JavaScript for one Bonjour service. */
data class BonjourService(
  val name: String,
  val host: String,
  val port: Int,
  val addrs: List<String>,
)

/** Pure rules shared by the Android Bonjour bridge and its tests. */
object BonjourLogic {
  const val MAX_ADDRESSES = 4

  /** The service name returned by NSD is a local label, not a DNS host name. */
  fun hostForServiceName(name: String): String = "$name.local"

  /**
   * Returns unique, deterministic addresses with IPv4 entries first.
   *
   * NSD can report the same address more than once while an interface changes. Empty
   * values are ignored because they cannot be used by the native clients.
   */
  fun sortAndLimitAddresses(addresses: Iterable<String>): List<String> = addresses
    .asSequence()
    .map(String::trim)
    .filter(String::isNotEmpty)
    .distinct()
    .sortedWith(
      compareBy<String> { if (isIpv4(it)) 0 else 1 }
        .thenBy { it },
    )
    .take(MAX_ADDRESSES)
    .toList()

  private fun isIpv4(address: String): Boolean {
    val octets = address.split('.')
    if (octets.size != 4) return false
    return octets.all { octet ->
      octet.isNotEmpty() && octet.length <= 3 && octet.all(Char::isDigit) &&
        octet.toIntOrNull()?.let { it in 0..255 } == true
    }
  }
}

/**
 * FIFO queue for pre-34 NSD resolution.
 *
 * Android only permits one active resolve operation at a time on the older API. A key
 * may occur once in the active slot or pending queue; a second discovery callback is
 * therefore harmless.
 */
class BonjourResolveQueue<K : Any> {
  private val pending = ArrayDeque<K>()
  private val pendingKeys = LinkedHashSet<K>()
  private var activeKey: K? = null

  val active: K?
    get() = activeKey

  val pendingCount: Int
    get() = pending.size

  fun enqueue(key: K): Boolean {
    if (activeKey == key || !pendingKeys.add(key)) return false
    pending.addLast(key)
    return true
  }

  /** Moves the next key into the active slot, or returns null while one is active. */
  fun startNext(): K? {
    if (activeKey != null) return null
    val next = pending.removeFirstOrNull() ?: return null
    pendingKeys.remove(next)
    activeKey = next
    return next
  }

  /** Completes the active key. A stale callback cannot complete a newer operation. */
  fun complete(key: K): Boolean {
    if (activeKey != key) return false
    activeKey = null
    return true
  }

  fun removePending(key: K): Boolean {
    if (!pendingKeys.remove(key)) return false
    pending.remove(key)
    return true
  }

  fun clear() {
    pending.clear()
    pendingKeys.clear()
    activeKey = null
  }
}

/** Suppresses identical found events while still allowing changed address information. */
class BonjourServiceDeduplicator<K : Any> {
  private val emitted = mutableMapOf<K, BonjourService>()

  fun shouldEmitFound(key: K, service: BonjourService): Boolean {
    if (emitted[key] == service) return false
    emitted[key] = service
    return true
  }

  fun remove(key: K): BonjourService? = emitted.remove(key)

  fun clear() {
    emitted.clear()
  }
}
