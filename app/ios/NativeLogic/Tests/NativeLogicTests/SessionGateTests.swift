import XCTest
@testable import NativeLogic

final class SessionGateTests: XCTestCase {
    private func frame(_ id: UInt32, epoch: UInt32 = 1, keyframe: Bool = false,
                       parameterSets: Bool = false) -> SessionFrameMeta {
        SessionFrameMeta(frameId: id, epoch: epoch, keyframe: keyframe, hasParameterSets: parameterSets)
    }

    func testBacklogDropsAllAndRerequestsThenRecovers() {
        let gate = SessionFrameGate(maxBacklog: 2, rerequestAfter: 0.5)
        XCTAssertEqual(gate.push(frame(1, keyframe: true, parameterSets: true), now: 0), [.decode(1)])
        XCTAssertEqual(gate.push(frame(2), now: 0.01), [.decode(2)])
        XCTAssertEqual(gate.push(frame(3), now: 0.02), [
            .drop(1), .drop(2), .drop(3), .requestKeyframe(.backlog),
        ])
        XCTAssertEqual(gate.push(frame(4), now: 0.1), [.drop(4)])
        XCTAssertEqual(gate.tick(now: 0.5), [])
        XCTAssertNil(gate.next(now: 0.5))
        XCTAssertEqual(gate.tick(now: 0.62), [.requestKeyframe(.backlog)])
        XCTAssertEqual(gate.push(frame(5, keyframe: true, parameterSets: true), now: 0.63), [.decode(5)])
        XCTAssertFalse(gate.waitingForKeyframe)
        XCTAssertEqual(gate.next(now: 0.63)?.frameId, 5)
        XCTAssertNil(gate.next(now: 0.63))
        XCTAssertEqual(gate.tick(now: 2), [])
        XCTAssertEqual(gate.push(frame(6), now: 2), [.decode(6)])
    }

    func testDecoderNotReadyDropsDeltaButAcceptsParameterizedKeyframe() {
        let gate = SessionFrameGate()
        gate.decoderReady(false)
        XCTAssertEqual(gate.push(frame(1), now: 1), [.drop(1), .requestKeyframe(.backlog)])
        XCTAssertEqual(gate.push(frame(2, keyframe: true, parameterSets: true), now: 1.01), [.decode(2)])
        XCTAssertEqual(gate.next(now: 1.01)?.frameId, 2)
    }

    func testDecodeFailureIsThrottledAndTickRerequests() {
        let gate = SessionFrameGate()
        XCTAssertEqual(gate.decodeFailed(frameId: 7, now: 0), [.requestKeyframe(.decodeError)])
        XCTAssertEqual(gate.decodeFailed(frameId: 8, now: 0.2), [])
        XCTAssertEqual(gate.tick(now: 0.49), [])
        XCTAssertEqual(gate.tick(now: 0.5), [.requestKeyframe(.decodeError)])
    }

    func testResetRequestsBacklogAfter500msUntilParameterizedKeyframe() {
        let gate = SessionFrameGate()
        _ = gate.push(frame(9), now: 0)
        gate.reset(waitForKeyframe: true, now: 0)
        XCTAssertEqual(gate.count, 0)
        XCTAssertEqual(gate.push(frame(1), now: 0), [.drop(1)])
        XCTAssertEqual(gate.push(frame(2, keyframe: true), now: 0.1), [.drop(2)])
        XCTAssertEqual(gate.tick(now: 0.49), [])
        XCTAssertEqual(gate.tick(now: 0.5), [.requestKeyframe(.backlog)])
        XCTAssertEqual(gate.push(frame(3, epoch: 2, keyframe: true, parameterSets: true), now: 0.6), [.decode(3)])
        XCTAssertEqual(gate.next(now: 0.6)?.epoch, 2)
    }

    func testFailureClearsQueuedDeltasAndKeepsTheRequestThrottleAcrossEpochs() {
        let gate = SessionFrameGate()
        _ = gate.push(frame(1), now: 0)
        _ = gate.next(now: 0)
        _ = gate.push(frame(2), now: 0)
        XCTAssertEqual(gate.decodeFailed(frameId: 1, now: 1), [.drop(2), .requestKeyframe(.decodeError)])
        gate.reset(waitForKeyframe: true)
        XCTAssertEqual(gate.tick(now: 1.49), [])
        XCTAssertEqual(gate.tick(now: 1.5), [.requestKeyframe(.backlog)])
        _ = gate.push(frame(3, epoch: 2, keyframe: true, parameterSets: true), now: 1.6)
        _ = gate.next(now: 1.6)
        XCTAssertEqual(gate.decodeFailed(frameId: 3, now: 1.7), [])
        XCTAssertEqual(gate.decodeFailed(frameId: 3, now: 1.8), [.requestKeyframe(.decodeError)])
    }

    func testUnavailableDecoderRequestsBacklogAndShortRetryStillRespects300ms() {
        let gate = SessionFrameGate(rerequestAfter: 0.1)
        _ = gate.push(frame(1, keyframe: true, parameterSets: true), now: 0)
        XCTAssertEqual(gate.decoderUnavailable(now: 1), [.drop(1), .requestKeyframe(.backlog)])
        XCTAssertEqual(gate.tick(now: 1.2), [])
        XCTAssertEqual(gate.tick(now: 1.3), [.requestKeyframe(.backlog)])
        XCTAssertEqual(SessionKeyframeReason.backlog.wireValue, 4)
        XCTAssertEqual(SessionKeyframeReason.decodeError.wireValue, 2)
        XCTAssertEqual(SessionKeyframeReason.decodeError.wireName, "decoder_reset")
    }

    func testDecodePercentilesAndOneSecondFpsWindow() {
        let stats = SessionDecodeStats()
        for value in 1...120 {
            stats.recordDecode(durationMs: Double(value), at: 0)
        }
        XCTAssertEqual(stats.snapshot(at: 0).decodeP50Ms, 60)
        XCTAssertEqual(stats.snapshot(at: 0).decodeP95Ms, 114)
        XCTAssertEqual(stats.snapshot(at: 0).latencyP50Ms, -1)

        stats.recordPresented(at: 0.1)
        stats.recordPresented(at: 0.5)
        stats.recordPresented(at: 1.2)
        XCTAssertEqual(stats.snapshot(at: 1.2).fps, 2)
        XCTAssertEqual(stats.snapshot(at: 1.6).fps, 1)
        XCTAssertEqual(stats.snapshot(at: 2.2).fps, 0) // (now - 1, now].
    }

    func testSampleWindowsAreIndependentAndReportsUseTheLatestFramesClockState() {
        let stats = SessionDecodeStats()
        for value in 1...121 {
            stats.recordFrame(decodeMs: Double(value), latencyMs: Double(value), presentedAt: nil)
        }
        let window = stats.snapshot(at: 0)
        XCTAssertEqual(window.decodeP50Ms, 61)
        XCTAssertEqual(window.decodeP95Ms, 115)
        XCTAssertEqual(window.latencyP50Ms, 61)
        XCTAssertEqual(window.latestDecodeUs, 121_000)
        XCTAssertEqual(window.latestLatencyUs, 121_000)
        stats.recordFrame(decodeMs: 2, latencyMs: nil, presentedAt: nil)
        XCTAssertEqual(stats.snapshot(at: 0).latencyP50Ms, 61)
        XCTAssertNil(stats.snapshot(at: 0).latestLatencyUs)
        stats.reset()
        XCTAssertEqual(stats.snapshot(at: 0).decodeP50Ms, -1)
        XCTAssertEqual(stats.snapshot(at: 0).latencyP50Ms, -1)
        XCTAssertNil(stats.snapshot(at: 0).latestDecodeUs)
    }
}
