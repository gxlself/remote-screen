package ai.gxlself.remotescreen.logic.terminal

import java.util.ArrayDeque

class TermOutputCoalescer {
    private data class Chunk(val data: ByteArray, var offset: Int = 0)

    private val lock = Any()
    private val chunks = ArrayDeque<Chunk>()
    private var pending = 0

    val pendingBytes: Int
        get() = synchronized(lock) { pending }

    fun append(data: ByteArray) {
        if (data.isEmpty()) return
        synchronized(lock) {
            chunks.addLast(Chunk(data.copyOf()))
            pending += data.size
        }
    }

    fun take(max: Int): ByteArray? {
        if (max <= 0) return null
        return synchronized(lock) {
            if (pending == 0) return@synchronized null

            val amount = minOf(max, pending)
            val result = ByteArray(amount)
            var destinationOffset = 0
            while (destinationOffset < amount) {
                val chunk = chunks.first()
                val count = minOf(amount - destinationOffset, chunk.data.size - chunk.offset)
                chunk.data.copyInto(
                    destination = result,
                    destinationOffset = destinationOffset,
                    startIndex = chunk.offset,
                    endIndex = chunk.offset + count,
                )
                destinationOffset += count
                chunk.offset += count
                pending -= count
                if (chunk.offset == chunk.data.size) chunks.removeFirst()
            }
            result
        }
    }
}
