import XCTest
@testable import NativeLogic

final class PipTests: XCTestCase {
    private func readyMachine(appActive: Bool = true) -> PipMachine {
        var machine = PipMachine(supported: true, appActive: appActive)
        XCTAssertEqual(
            machine.handle(.inputs(PipInputs(canStart: true, canContinue: true, surfaceReady: true))),
            [.setAutoStart(true)]
        )
        XCTAssertEqual(machine.handle(.possibleChanged(true)), [])
        XCTAssertEqual(machine.phase, .armed)
        return machine
    }

    private func activeMachine(appActive: Bool = true) -> PipMachine {
        var machine = readyMachine(appActive: appActive)
        _ = machine.handle(.willStart)
        _ = machine.handle(.didStart)
        XCTAssertEqual(machine.phase, .active)
        return machine
    }

    func testUnsupportedIgnoresEveryEvent() {
        var machine = PipMachine(supported: false, appActive: true)
        let events: [PipEvent] = [
            .inputs(PipInputs(canStart: true, canContinue: true, surfaceReady: true)),
            .possibleChanged(true), .userStart, .userStop, .willStart, .didStart,
            .failedToStart, .willStop, .didStop, .restoreRequested,
            .setPlaying(false), .appDidBecomeActive, .appDidEnterBackground,
        ]

        for event in events {
            XCTAssertEqual(machine.handle(event), [], "unexpected action for \(event)")
        }
        XCTAssertEqual(machine.phase, .unsupported)
    }

    func testInputsArmOnlyAfterPossibleChanged() {
        var machine = PipMachine(supported: true, appActive: true)
        XCTAssertEqual(
            machine.handle(.inputs(PipInputs(canStart: true, canContinue: true, surfaceReady: true))),
            [.setAutoStart(true)]
        )
        XCTAssertEqual(machine.phase, .idle)
        XCTAssertEqual(machine.handle(.possibleChanged(true)), [])
        XCTAssertEqual(machine.phase, .armed)
    }

    func testEligibilityLossDisarmsAndDisablesAutomaticStart() {
        var machine = readyMachine()
        XCTAssertEqual(
            machine.handle(.inputs(PipInputs(canStart: false, canContinue: true, surfaceReady: true))),
            [.setAutoStart(false)]
        )
        XCTAssertEqual(machine.phase, .idle)
    }

    func testUserStartFromArmedStartsAndDisablesAutomaticStart() {
        var machine = readyMachine()
        XCTAssertEqual(machine.handle(.userStart), [.start, .setAutoStart(false)])
        XCTAssertEqual(machine.phase, .starting)
        XCTAssertTrue(machine.isActive)
    }

    func testUserStartWhenIdleOrInBackgroundIsNotPossible() {
        var idle = PipMachine(supported: true, appActive: true)
        XCTAssertEqual(
            idle.handle(.userStart),
            [.emitChange(active: false, reason: "notPossible")]
        )

        var background = readyMachine()
        XCTAssertEqual(background.handle(.appDidEnterBackground), [])
        XCTAssertEqual(
            background.handle(.userStart),
            [.emitChange(active: false, reason: "notPossible")]
        )
    }

    func testAutomaticStartAndStartedCallbacks() {
        var machine = readyMachine()
        XCTAssertEqual(machine.handle(.willStart), [
            .setPassthrough(true),
            .emitChange(active: true, reason: "willStart"),
            .setAutoStart(false),
        ])
        XCTAssertEqual(machine.phase, .starting)
        XCTAssertEqual(machine.handle(.didStart), [.emitChange(active: true, reason: "started")])
        XCTAssertEqual(machine.phase, .active)
    }

    func testPlaybackPauseAndResumeOnlyChangeActivePip() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.setPlaying(false)), [
            .emitPlayback(paused: true),
            .invalidatePlaybackState,
        ])
        XCTAssertTrue(machine.paused)
        XCTAssertEqual(machine.handle(.setPlaying(false)), [])
        XCTAssertEqual(machine.handle(.setPlaying(true)), [
            .emitPlayback(paused: false),
            .invalidatePlaybackState,
        ])
        XCTAssertFalse(machine.paused)
    }

    func testIneligibleStopInBackgroundDefersPassthroughUntilForeground() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.appDidEnterBackground), [])
        XCTAssertEqual(
            machine.handle(.inputs(PipInputs(canStart: false, canContinue: false, surfaceReady: true))),
            [.stop]
        )
        XCTAssertEqual(machine.phase, .stopping)

        let stopped = machine.handle(.didStop)
        XCTAssertTrue(stopped.contains(.emitChange(active: false, reason: "ineligible")))
        XCTAssertFalse(stopped.contains(.setPassthrough(false)))
        XCTAssertEqual(machine.handle(.appDidBecomeActive), [.setPassthrough(false)])
    }

    func testDetachedStopReportsDetachedReason() {
        var machine = activeMachine()
        XCTAssertEqual(
            machine.handle(.inputs(PipInputs(canStart: true, canContinue: true, surfaceReady: false))),
            [.stop]
        )
        let stopped = machine.handle(.didStop)
        XCTAssertTrue(stopped.contains(.emitChange(active: false, reason: "detached")))
    }

    func testRestoreHasPriorityAndRearmsAutomaticStart() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.restoreRequested), [.completeRestore(true)])
        XCTAssertEqual(machine.handle(.appDidBecomeActive), [])
        XCTAssertEqual(machine.handle(.willStop), [])
        XCTAssertEqual(machine.handle(.didStop), [
            .emitChange(active: false, reason: "restore"),
            .setPassthrough(false),
            .setAutoStart(true),
        ])
    }

    func testCloseInBackgroundDoesNotDisablePassthrough() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.appDidEnterBackground), [])
        XCTAssertEqual(machine.handle(.willStop), [])
        let stopped = machine.handle(.didStop)
        XCTAssertTrue(stopped.contains(.emitChange(active: false, reason: "closed")))
        XCTAssertFalse(stopped.contains(.setPassthrough(false)))
    }

    func testPausedPipStopResumesPlaybackAndClearsPause() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.setPlaying(false)), [
            .emitPlayback(paused: true),
            .invalidatePlaybackState,
        ])
        XCTAssertEqual(machine.handle(.userStop), [.stop])
        let stopped = machine.handle(.didStop)
        XCTAssertTrue(stopped.contains(.emitChange(active: false, reason: "stopped")))
        XCTAssertTrue(stopped.contains(.emitPlayback(paused: false)))
        XCTAssertTrue(stopped.contains(.setPassthrough(false)))
        XCTAssertFalse(machine.paused)
    }

    func testForegroundAfterBackgroundStopsPip() {
        var machine = readyMachine()
        XCTAssertEqual(machine.handle(.appDidEnterBackground), [])
        _ = machine.handle(.willStart)
        _ = machine.handle(.didStart)
        XCTAssertEqual(machine.handle(.appDidBecomeActive), [.stop])
        XCTAssertEqual(machine.phase, .stopping)
        let stopped = machine.handle(.didStop)
        XCTAssertTrue(stopped.contains(.emitChange(active: false, reason: "foreground")))
    }

    func testForegroundWithoutBackgroundDoesNotStopManualPip() {
        var machine = activeMachine()
        XCTAssertEqual(machine.handle(.appDidBecomeActive), [])
    }

    func testFailedStartForegroundBackgroundAndWithoutWillStart() {
        var machine = readyMachine()
        _ = machine.handle(.willStart)
        let actions = machine.handle(.failedToStart)
        XCTAssertTrue(actions.contains(.setPassthrough(false)))
        XCTAssertTrue(actions.contains(.emitChange(active: false, reason: "failed")))
        XCTAssertEqual(machine.phase, .armed)

        var background = readyMachine()
        XCTAssertEqual(background.handle(.appDidEnterBackground), [])
        _ = background.handle(.willStart)
        let failed = background.handle(.failedToStart)
        XCTAssertFalse(failed.contains(.setPassthrough(false)))
        XCTAssertTrue(failed.contains(.emitChange(active: false, reason: "failed")))
        XCTAssertEqual(background.handle(.appDidBecomeActive), [.setPassthrough(false)])

        var withoutWillStart = readyMachine()
        _ = withoutWillStart.handle(.userStart)
        let noWillStartActions = withoutWillStart.handle(.failedToStart)
        XCTAssertFalse(noWillStartActions.contains(.setPassthrough(false)))
        XCTAssertTrue(noWillStartActions.contains(.emitChange(active: false, reason: "failed")))
    }

    func testPassthroughActionsNeverRepeatTheSameValue() {
        var machine = PipMachine(supported: true, appActive: true)
        var seed: UInt64 = 0x9e37_79b9_7f4a_7c15
        var previous: Bool?

        for _ in 0..<200 {
            seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            let value = seed % 13
            let event: PipEvent
            switch value {
            case 0:
                event = .inputs(PipInputs(
                    canStart: (seed & 1) != 0,
                    canContinue: (seed & 2) != 0,
                    surfaceReady: (seed & 4) != 0
                ))
            case 1:
                event = .possibleChanged((seed & 1) != 0)
            case 2: event = .userStart
            case 3: event = .userStop
            case 4: event = .willStart
            case 5: event = .didStart
            case 6: event = .failedToStart
            case 7: event = .willStop
            case 8: event = .didStop
            case 9: event = .restoreRequested
            case 10: event = .setPlaying((seed & 1) != 0)
            case 11: event = .appDidBecomeActive
            default: event = .appDidEnterBackground
            }

            for action in machine.handle(event) {
                guard case .setPassthrough(let enabled) = action else { continue }
                if let previousValue = previous {
                    XCTAssertNotEqual(previousValue, enabled)
                }
                previous = enabled
            }
        }
    }

    func testRenderSwitchRequestsAndRoutesAfterKeyframe() {
        var render = PipRenderSwitch()
        XCTAssertEqual(render.request(.decode), [])
        XCTAssertEqual(render.request(.passthrough), [
            .flushKeepingImage,
            .invalidateDecoder,
            .requestKeyframe,
        ])
        XCTAssertTrue(render.awaitingKeyframe)
        XCTAssertEqual(render.route(keyframe: false), .drop)
        XCTAssertEqual(render.route(keyframe: true), .passthrough)
        XCTAssertFalse(render.awaitingKeyframe)
        XCTAssertEqual(render.route(keyframe: false), .passthrough)
    }

    func testRenderSwitchResetForAttachStopsWaitingForKeyframe() {
        var render = PipRenderSwitch()
        _ = render.request(.passthrough)
        render.resetForAttach()
        XCTAssertFalse(render.awaitingKeyframe)
        XCTAssertEqual(render.route(keyframe: false), .passthrough)
    }
}
