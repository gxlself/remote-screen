import Foundation

public final class TerminalOutputCoalescer: @unchecked Sendable {
    private let lock = NSLock()
    private var chunks: [Data] = []
    private var head = 0
    private var headOffset = 0
    private var bytes = 0

    public init() {}

    public var pendingBytes: Int {
        lock.lock()
        defer { lock.unlock() }
        return bytes
    }

    public func append(_ data: Data) {
        guard !data.isEmpty else { return }
        lock.lock()
        chunks.append(data)
        bytes += data.count
        lock.unlock()
    }

    public func append(_ data: [UInt8]) {
        append(Data(data))
    }

    public func take(max limit: Int) -> Data? {
        guard limit > 0 else { return nil }
        lock.lock()
        defer { lock.unlock() }
        guard bytes > 0 else { return nil }

        let amount = min(limit, bytes)
        var result = Data()
        result.reserveCapacity(amount)
        var remaining = amount
        while remaining > 0, head < chunks.count {
            let chunk = chunks[head]
            let available = chunk.count - headOffset
            let count = min(remaining, available)
            let start = chunk.startIndex + headOffset
            result.append(chunk[start..<(start + count)])
            headOffset += count
            remaining -= count
            bytes -= count
            if headOffset == chunk.count {
                head += 1
                headOffset = 0
            }
        }
        if head == chunks.count {
            chunks.removeAll(keepingCapacity: true)
            head = 0
        } else if head > 32 && head * 2 > chunks.count {
            chunks.removeFirst(head)
            head = 0
        }
        return result
    }
}
