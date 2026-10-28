import Foundation

struct AdapterRawSlot: Equatable {
    let index: Int
    let sinceSeq: UInt64
    let needsAlloc: Bool
}

/// Rotates three independently tracked pixel buffers. A slot is invalidated
/// whenever allocation fails or a copy does not return a sequence.
struct AdapterRawRing {
    private struct Entry {
        var width: Int = 0
        var height: Int = 0
        var seq: UInt64 = 0
        var valid = false
    }

    private var entries: [Entry]
    private var cursor = -1

    init(count: Int = 3) {
        entries = Array(repeating: Entry(), count: max(1, count))
    }

    mutating func next(width: Int, height: Int) -> AdapterRawSlot {
        cursor = (cursor + 1) % entries.count
        var entry = entries[cursor]
        let sizeChanged = entry.width != width || entry.height != height
        if sizeChanged {
            entry.width = width
            entry.height = height
            entry.seq = 0
            entry.valid = false
            entries[cursor] = entry
        }
        return AdapterRawSlot(index: cursor, sinceSeq: entry.valid ? entry.seq : 0,
                              needsAlloc: !entry.valid)
    }

    mutating func copied(index: Int, seq: UInt64) {
        guard entries.indices.contains(index) else { return }
        if seq == 0 {
            entries[index].seq = 0
            entries[index].valid = false
        } else {
            entries[index].seq = seq
            entries[index].valid = true
        }
    }

    mutating func reset() {
        entries = Array(repeating: Entry(), count: entries.count)
        cursor = -1
    }
}
