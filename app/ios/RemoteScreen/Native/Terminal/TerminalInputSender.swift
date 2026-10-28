import Foundation

/// All producers run on the main thread. Only owned bytes and a destination
/// generation cross to the serial ABI queue; UIKit is never loaded there.
final class TerminalInputSender: @unchecked Sendable {
    private let queue = DispatchQueue(label: "rs.term.input")
    @MainActor private weak var surface: RsTerminalSurface?
    // The destination and generation are the only state used on both queues.
    private let lock = NSLock()
    private var sshId: UInt64?
    private var debugEcho = false
    private var generation: UInt64 = 0

    @MainActor init(surface: RsTerminalSurface) {
        self.surface = surface
    }

    func setDestination(sshId: UInt64?, debugEcho: Bool) {
        lock.lock()
        defer { lock.unlock() }
        guard self.sshId != sshId || self.debugEcho != debugEcho else { return }
        generation &+= 1
        self.sshId = sshId
        self.debugEcho = debugEcho
    }

    func invalidate() {
        lock.lock()
        generation &+= 1
        sshId = nil
        debugEcho = false
        lock.unlock()
    }

    @MainActor func send(sourceData data: ArraySlice<UInt8>, modifiers: Int32) {
        let applied = TerminalModifierApply.apply(bytes: Array(data), mods: modifiers)
        if applied.consumed { surface?.modifiersConsumed(modifiers) }
        sendRaw(applied.bytes)
    }

    func sendSequence(_ text: String) {
        sendRaw(Array(text.utf8))
    }

    @MainActor func sendKey(_ name: String, modifiers: Int32) {
        guard let surface else { return }
        let bytes = TerminalKeySequences.bytes(name: name, appCursor: surface.applicationCursor, mods: modifiers)
        guard !bytes.isEmpty else { return }
        if modifiers != 0 { surface.modifiersConsumed(modifiers) }
        sendRaw(bytes)
    }

    func sendRaw(_ bytes: [UInt8]) {
        guard !bytes.isEmpty else { return }
        lock.lock()
        let expected = generation
        let id = sshId
        #if DEBUG
        let echo = debugEcho
        #endif
        lock.unlock()
        queue.async { [self] in
            lock.lock()
            guard expected == generation else { lock.unlock(); return }
            if let id {
                // Input copies synchronously in Rust. Serialize destination
                // invalidation with this call, so queued keys cannot leak to a
                // detached or recycled terminal.
                bytes.withUnsafeBufferPointer { buffer in
                    rs_term_input(id, buffer.baseAddress, buffer.count)
                }
                lock.unlock()
                return
            }
            lock.unlock()
            #if DEBUG
            if echo {
                var echoed: [UInt8] = []
                echoed.reserveCapacity(bytes.count)
                for byte in bytes {
                    echoed.append(byte)
                    if byte == 0x0d { echoed.append(0x0a) }
                }
                let echoBytes = echoed
                RsLog.marker("TERMVIEW-INPUT", ["hex": Self.hex(bytes)])
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.lock.lock()
                    let current = expected == self.generation
                    self.lock.unlock()
                    if current { self.surface?.feedLocal(echoBytes) }
                }
            }
            #endif
        }
    }

    #if DEBUG
    private static func hex(_ bytes: [UInt8]) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }
    #endif
}
