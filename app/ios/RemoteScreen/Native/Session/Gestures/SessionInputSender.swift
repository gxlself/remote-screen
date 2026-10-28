import QuartzCore

/// Lifecycle calls are enqueued with input, so an old session's button-up and
/// ReleaseAll precede the next session's input without waiting on the UI thread.
final class SessionInputSender {
    private static let queue = DispatchQueue(label: "rs.session.input", qos: .userInteractive)
    private var sessionId: UInt64?
    private var throttle = TrackpadPointerThrottle()
    private var pointerWork: DispatchWorkItem?
    private var pointerGeneration: UInt64 = 0

    func setSession(_ id: UInt64?) {
        Self.queue.async { [self] in
            guard sessionId != id else { return }
            cancelPointerWork()
            throttle = TrackpadPointerThrottle()
            sessionId = id
        }
    }

    func pointer(_ point: CGPoint) {
        Self.queue.async { [self] in
            guard sessionId != nil else { return }
            if let ready = throttle.offer(point, now: CACurrentMediaTime()) {
                cancelPointerWork()
                sendPointer(ready)
            } else {
                schedulePointerFlush()
            }
        }
    }

    func flushPointer() {
        Self.queue.async { [self] in schedulePointerFlush() }
    }

    func button(_ button: Int32, down: Bool, at point: CGPoint, clicks: UInt8 = 1) {
        Self.queue.async { [self] in
            guard let sessionId else { return }
            rs_input_button(sessionId, button, down, Float(point.x), Float(point.y), clicks)
        }
    }

    func click(_ button: Int32, at point: CGPoint, clicks: UInt8 = 1) {
        Self.queue.async { [self] in
            guard let sessionId else { return }
            rs_input_button(sessionId, button, true, Float(point.x), Float(point.y), clicks)
            rs_input_button(sessionId, button, false, Float(point.x), Float(point.y), clicks)
        }
    }

    func scroll(dx: CGFloat, dy: CGFloat, phase: UInt8 = 0, momentum: UInt8 = 0) {
        Self.queue.async { [self] in
            guard let sessionId else { return }
            rs_input_scroll(sessionId, Float(dx), Float(dy), phase, momentum)
        }
    }

    func releaseAll() {
        Self.queue.async { [self] in
            cancelPointerWork()
            _ = throttle.flush(now: CACurrentMediaTime())
            guard let sessionId else { return }
            rs_input_release_all(sessionId)
        }
    }

    private func sendPointer(_ point: CGPoint) {
        guard let sessionId else { return }
        rs_input_pointer(sessionId, Float(point.x), Float(point.y))
    }

    private func schedulePointerFlush() {
        guard sessionId != nil, throttle.hasPending, pointerWork == nil else { return }
        let now = CACurrentMediaTime()
        let deadline = (throttle.lastEmissionTime ?? now) + TrackpadPointerThrottle.interval
        let expected = pointerGeneration
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.pointerGeneration == expected else { return }
            self.pointerWork = nil
            let now = CACurrentMediaTime()
            if now < deadline {
                self.schedulePointerFlush()
            } else if let point = self.throttle.flush(now: now) {
                self.sendPointer(point)
            }
        }
        pointerWork = work
        Self.queue.asyncAfter(deadline: .now() + max(0, deadline - now), execute: work)
    }

    private func cancelPointerWork() {
        pointerGeneration &+= 1
        pointerWork?.cancel()
        pointerWork = nil
    }
}
