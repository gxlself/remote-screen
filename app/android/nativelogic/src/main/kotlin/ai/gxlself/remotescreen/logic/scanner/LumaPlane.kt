package ai.gxlself.remotescreen.logic.scanner

/** Copies a CameraX Y plane into a tightly packed width-by-height array. */
object LumaPlane {
  fun pack(
    src: ByteArray,
    rowStride: Int,
    pixelStride: Int,
    width: Int,
    height: Int,
  ): ByteArray {
    require(rowStride >= 0) { "rowStride must be non-negative" }
    require(pixelStride > 0) { "pixelStride must be positive" }
    require(width >= 0 && height >= 0) { "dimensions must be non-negative" }
    val packed = ByteArray(width * height)
    for (row in 0 until height) {
      val sourceRow = row * rowStride
      val targetRow = row * width
      for (column in 0 until width) {
        val sourceIndex = sourceRow + column * pixelStride
        packed[targetRow + column] = src.getOrElse(sourceIndex) { 0 }
      }
    }
    return packed
  }
}
