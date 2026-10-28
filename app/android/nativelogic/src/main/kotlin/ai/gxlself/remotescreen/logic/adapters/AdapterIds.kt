package ai.gxlself.remotescreen.logic.adapters

/** Adapter (VNC / RDP) session IDs start at RS_ADAPTER_ID_BASE in rs_mobile.h. */
object AdapterIds {
  const val BASE: Long = 1L shl 62

  /** Unsigned comparison: id ≥ 2^62, including ids ≥ 2^63 that are negative Longs. */
  fun isAdapterId(id: Long): Boolean = java.lang.Long.compareUnsigned(id, BASE) >= 0
}
