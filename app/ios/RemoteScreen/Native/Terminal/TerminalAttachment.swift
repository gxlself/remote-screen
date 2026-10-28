import Foundation

/// Each attachment owns its queue. The callback never loads the weak surface
/// on a worker thread, so UIKit teardown cannot run on the last callback's stack.
private final class TerminalCallbackBox: @unchecked Sendable {
    // Only drain (on the main actor) reads this reference. All queue and
    // scheduling state accessed by the worker callback is protected by lock.
    @MainActor private weak var surface: RsTerminalSurface?
    private let output = TerminalOutputCoalescer()
    private let lock = NSLock()
    private var active = true
    private var refreshScheduled = false
    private var backlogLogged = false
    private static let refreshLimit = 256 * 1024

    @MainActor init(surface: RsTerminalSurface) {
        self.surface = surface
    }

    func append(_ data: Data) {
        lock.lock()
        guard active else { lock.unlock(); return }
        output.append(data)
        let schedule = !refreshScheduled
        refreshScheduled = true
        lock.unlock()
        if schedule { DispatchQueue.main.async { [self] in drain() } }
    }

    func invalidate() {
        lock.lock()
        active = false
        lock.unlock()
    }

    @MainActor private func drain() {
        dispatchPrecondition(condition: .onQueue(.main))
        lock.lock()
        guard active else { lock.unlock(); return }
        let backlog = output.pendingBytes
        let logBacklog = backlog > 1024 * 1024 && !backlogLogged
        if logBacklog { backlogLogged = true }
        let data = output.take(max: Self.refreshLimit)
        lock.unlock()

        if logBacklog { RsLog.marker("TERMVIEW", ["backlog": backlog]) }
        if let data { surface?.feedOutput(data) }

        lock.lock()
        guard active else { lock.unlock(); return }
        // Check pending bytes and disarm under the same lock used by append.
        // Otherwise a callback could append between the empty check and disarm.
        let more = output.pendingBytes > 0
        if !more {
            refreshScheduled = false
            backlogLogged = false
        }
        lock.unlock()
        if more { DispatchQueue.main.async { [self] in drain() } }
    }
}

private let terminalOutputCallback: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void = {
    context, bytes, length in
    guard let context, let bytes, length > 0 else { return }
    let box = Unmanaged<TerminalCallbackBox>.fromOpaque(context).takeUnretainedValue()
    // Pointers are borrowed only for this call. Never wait for the main thread:
    // detach waits for this callback to return.
    box.append(Data(bytes: bytes, count: length))
}

/// All lifecycle and resize calls run on the main thread. The ABI's detach
/// barrier protects the retained box until all worker callbacks have returned.
@MainActor final class TerminalAttachment {
    private static let maxAttempts = 10
    private static let retryDelay: TimeInterval = 0.5
    private var attachedId: UInt64?
    private var requestedValue: String?
    private var context: UnsafeMutableRawPointer?
    private var retry: DispatchWorkItem?
    private var generation: UInt64 = 0

    func attach(_ value: String, surface: RsTerminalSurface) {
        guard requestedValue != value else { return }
        detach()
        requestedValue = value
        guard !value.isEmpty else { return }
        guard let id = SessionIdParse.parse(value) else {
            RsLog.marker("TERMVIEW", ["attach": false, "sshId": value])
            return
        }
        attempt(id: id, value: value, surface: surface, number: 1, generation: generation)
    }

    func detach() {
        generation &+= 1
        retry?.cancel()
        retry = nil
        requestedValue = nil
        if let id = attachedId { rs_term_detach(id) }
        attachedId = nil
        if let context {
            let box = Unmanaged<TerminalCallbackBox>.fromOpaque(context)
            box.takeUnretainedValue().invalidate()
            // No callback can use ctx after the detach barrier above.
            box.release()
        }
        context = nil
    }

    func resize(cols: Int, rows: Int) {
        guard let id = attachedId, cols > 0, rows > 0 else { return }
        rs_term_resize(id, UInt16(clamping: cols), UInt16(clamping: rows))
    }

    private func attempt(id: UInt64, value: String, surface: RsTerminalSurface,
                         number: Int, generation expected: UInt64) {
        guard expected == generation else { return }
        retry = nil
        let box = Unmanaged.passRetained(TerminalCallbackBox(surface: surface))
        let context = box.toOpaque()
        if rs_term_attach(id, context, terminalOutputCallback) {
            attachedId = id
            self.context = context
            RsLog.marker("TERMVIEW", ["attach": true])
            surface.terminalDidAttach(id)
            return
        }

        // A failed attach retains neither consumer nor callback context.
        box.takeUnretainedValue().invalidate()
        box.release()
        guard number < Self.maxAttempts else {
            RsLog.marker("TERMVIEW", ["attach": false, "sshId": value])
            return
        }
        let work = DispatchWorkItem { [weak self, weak surface] in
            guard let self, let surface else { return }
            self.attempt(id: id, value: value, surface: surface,
                         number: number + 1, generation: expected)
        }
        retry = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.retryDelay, execute: work)
    }

    deinit {
        // The Fabric surface owns this object and is torn down on the main queue.
        MainActor.assumeIsolated { detach() }
    }
}
