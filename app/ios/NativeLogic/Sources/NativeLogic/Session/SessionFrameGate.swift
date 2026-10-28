import Foundation

/// The reason attached to a local keyframe request. LOSS is determined by the
/// Rust receiver; the App only requests recovery for decoder reset and backlog.
public enum SessionKeyframeReason: Equatable {
    case backlog
    case decodeError

    public var wireName: String {
        switch self {
        case .backlog: return "backlog"
        case .decodeError: return "decoder_reset"
        }
    }

    /// proto/rsp/v1/control.proto, KeyframeRequest.Reason.
    public var wireValue: UInt8 {
        switch self {
        case .backlog: return 4
        case .decodeError: return 2
        }
    }
}

public struct SessionFrameMeta: Equatable {
    public let frameId: UInt32
    public let epoch: UInt32
    public let keyframe: Bool
    public let hasParameterSets: Bool

    public init(frameId: UInt32, epoch: UInt32, keyframe: Bool, hasParameterSets: Bool) {
        self.frameId = frameId
        self.epoch = epoch
        self.keyframe = keyframe
        self.hasParameterSets = hasParameterSets
    }
}

public enum SessionGateAction: Equatable {
    case decode(UInt32)
    case drop(UInt32)
    case requestKeyframe(SessionKeyframeReason)
}

/// Serial, deterministic receive-side policy for video frame metadata.
///
/// The payload queue is owned by the caller. This type only queues metadata,
/// which keeps the policy testable without VideoToolbox or a clock dependency.
public final class SessionFrameGate {
    private let maxBacklog: Int
    private let rerequestAfter: TimeInterval
    private let minimumRequestInterval: TimeInterval = 0.3

    private var items: [SessionFrameMeta] = []
    private var waiting = false
    private var waitingReason: SessionKeyframeReason?
    private var resetWaitingSince: TimeInterval?
    private var decoderIsReady = true
    private var lastRequestAt: TimeInterval?
    private var lastObservedNow: TimeInterval?

    public init(maxBacklog: Int = 2, rerequestAfter: TimeInterval = 0.5) {
        self.maxBacklog = max(1, maxBacklog)
        self.rerequestAfter = max(0, rerequestAfter)
    }

    public var count: Int { items.count }
    public var waitingForKeyframe: Bool { waiting }

    /// Makes the enqueue decision for one frame. An accepted frame remains in
    /// the metadata queue until `next` is called by the decode loop.
    public func push(_ m: SessionFrameMeta, now: TimeInterval) -> [SessionGateAction] {
        lastObservedNow = now
        if waiting {
            guard m.keyframe, m.hasParameterSets else {
                return [.drop(m.frameId)]
            }
            waiting = false
            waitingReason = nil
            resetWaitingSince = nil
            items.append(m)
            return [.decode(m.frameId)]
        }

        // A delta cannot make progress while the decoder has no usable
        // session. Clear any stale metadata and wait for a recovery keyframe.
        if !decoderIsReady && (!m.keyframe || !m.hasParameterSets) {
            let dropped = items.map(\.frameId) + [m.frameId]
            items.removeAll(keepingCapacity: true)
            waiting = true
            waitingReason = .backlog
            resetWaitingSince = nil
            var actions = dropped.map(SessionGateAction.drop)
            if let request = requestIfAllowed(reason: .backlog, now: now) {
                actions.append(request)
            }
            return actions
        }

        items.append(m)
        guard items.count <= maxBacklog else {
            let dropped = items.map(\.frameId)
            items.removeAll(keepingCapacity: true)
            waiting = true
            waitingReason = .backlog
            resetWaitingSince = nil
            var actions = dropped.map(SessionGateAction.drop)
            if let request = requestIfAllowed(reason: .backlog, now: now) {
                actions.append(request)
            }
            return actions
        }

        return [.decode(m.frameId)]
    }

    /// Removes and returns the oldest metadata item for the decode worker.
    public func next(now: TimeInterval) -> SessionFrameMeta? {
        lastObservedNow = now
        guard !waiting, !items.isEmpty else { return nil }
        return items.removeFirst()
    }

    public func decoderReady(_ ready: Bool) {
        decoderIsReady = ready
    }

    /// Marks the current frame as unusable and waits for a parameterized
    /// keyframe. Metadata that was waiting behind the failed frame is stale and
    /// is dropped as part of the same recovery transition.
    public func decodeFailed(frameId: UInt32, now: TimeInterval) -> [SessionGateAction] {
        lastObservedNow = now
        return enterRecovery(reason: .decodeError, now: now)
    }

    /// A failed VT session creation cannot decode even a keyframe. Treat it
    /// as decoder-not-ready/backlog recovery rather than a corrupt delta.
    public func decoderUnavailable(now: TimeInterval) -> [SessionGateAction] {
        lastObservedNow = now
        decoderIsReady = false
        return enterRecovery(reason: .backlog, now: now)
    }

    /// Re-requests only after the configured quiet period and the common local
    /// 300 ms request throttle. A reset-induced wait starts a BACKLOG recovery
    /// cycle only when no parameterized keyframe arrived in time.
    public func tick(now: TimeInterval) -> [SessionGateAction] {
        lastObservedNow = now
        if waiting, waitingReason == nil, let resetWaitingSince,
           now >= resetWaitingSince + rerequestAfter {
            if let request = requestIfAllowed(reason: .backlog, now: now) {
                self.resetWaitingSince = nil
                return [request]
            }
            return []
        }
        guard waiting, let reason = waitingReason, let lastRequestAt else { return [] }
        guard now >= lastRequestAt + rerequestAfter else { return [] }
        if let request = requestIfAllowed(reason: reason, now: now) {
            return [request]
        }
        return []
    }

    /// Epoch changes and attachment resets use this transition. It initially
    /// stays quiet because the first frame of a fresh epoch is expected to be a
    /// keyframe; `tick` requests BACKLOG after the configured wait if it never
    /// arrives. The overload with `now` lets native callers provide the exact
    /// reset timestamp while preserving the original public call shape.
    public func reset(waitForKeyframe: Bool) {
        reset(waitForKeyframe: waitForKeyframe, now: lastObservedNow ?? 0)
    }

    public func reset(waitForKeyframe: Bool, now: TimeInterval) {
        lastObservedNow = now
        items.removeAll(keepingCapacity: true)
        waiting = waitForKeyframe
        waitingReason = nil
        resetWaitingSince = waitForKeyframe ? now : nil
        // Preserve the local throttle across epoch resets. A new gate is
        // created for each attach, where the cumulative counters also reset.
    }

    private func enterRecovery(reason: SessionKeyframeReason, now: TimeInterval) -> [SessionGateAction] {
        lastObservedNow = now
        let dropped = items.map(\.frameId)
        items.removeAll(keepingCapacity: true)
        waiting = true
        waitingReason = reason
        resetWaitingSince = nil
        var actions = dropped.map(SessionGateAction.drop)
        if let request = requestIfAllowed(reason: reason, now: now) {
            actions.append(request)
        }
        return actions
    }

    private func requestIfAllowed(reason: SessionKeyframeReason, now: TimeInterval) -> SessionGateAction? {
        if let lastRequestAt, now < lastRequestAt + minimumRequestInterval {
            return nil
        }
        lastRequestAt = now
        waitingReason = reason
        return .requestKeyframe(reason)
    }
}
