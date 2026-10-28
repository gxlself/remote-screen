import XCTest
@testable import NativeLogic

final class AudioTests: XCTestCase {
    func testStartFromIdleActivatesOpensAndStarts() {
        var machine = AudioPlaybackMachine()
        XCTAssertEqual(
            machine.handle(.start("A")),
            [.activateSession, .openAudio("A"), .startEngine,
             .emit(state: .running("A"), reason: "start")]
        )
        XCTAssertEqual(machine.state, .running("A"))
    }

    func testStartingAnotherSessionClosesTheCurrentHandle() {
        var machine = AudioPlaybackMachine()
        _ = machine.handle(.start("A"))
        let actions = machine.handle(.start("B"))
        XCTAssertEqual(
            actions,
            [.stopEngine, .closeAudio, .activateSession, .openAudio("B"), .startEngine,
             .emit(state: .running("B"), reason: "start")]
        )
        XCTAssertEqual(machine.state, .running("B"))
    }

    func testInterruptionStopsAndResumes() {
        var machine = AudioPlaybackMachine()
        _ = machine.handle(.start("A"))
        XCTAssertEqual(
            machine.handle(.interruptionBegan),
            [.stopEngine, .emit(state: .interrupted("A"), reason: "interruption")]
        )
        XCTAssertEqual(
            machine.handle(.interruptionEnded(shouldResume: true)),
            [.activateSession, .startEngine,
             .emit(state: .running("A"), reason: "interruptionEnded")]
        )
    }

    func testInterruptionWithoutResumeStaysInterrupted() {
        var machine = AudioPlaybackMachine(state: .interrupted("A"))
        XCTAssertEqual(machine.handle(.interruptionEnded(shouldResume: false)), [])
        XCTAssertEqual(machine.state, .interrupted("A"))
    }

    func testBackgroundStopsAndForegroundResumes() {
        var machine = AudioPlaybackMachine()
        _ = machine.handle(.start("A"))
        XCTAssertEqual(
            machine.handle(.enterBackground),
            [.stopEngine, .deactivateSession,
             .emit(state: .background("A"), reason: "background")]
        )
        XCTAssertEqual(
            machine.handle(.enterForeground),
            [.activateSession, .startEngine,
             .emit(state: .running("A"), reason: "foreground")]
        )
    }

    func testConfigurationOnlyRestartsRunningEngine() {
        var machine = AudioPlaybackMachine()
        XCTAssertEqual(machine.handle(.configurationChanged), [])
        _ = machine.handle(.start("A"))
        XCTAssertEqual(machine.handle(.configurationChanged), [.startEngine])
    }

    func testMediaServicesResetRebuildsRunningEngine() {
        var machine = AudioPlaybackMachine()
        _ = machine.handle(.start("A"))
        let actions = machine.handle(.mediaServicesReset)
        XCTAssertEqual(actions, [.rebuildEngine, .activateSession, .startEngine])
    }

    func testEngineStartFailureEmitsFailedState() {
        var machine = AudioPlaybackMachine(state: .running("A"))
        XCTAssertEqual(
            machine.handle(.engineStartFailed("x")),
            [.stopEngine, .closeAudio, .deactivateSession,
             .emit(state: .failed("A", reason: "x"), reason: "x")]
        )
        XCTAssertEqual(machine.state, .failed("A", reason: "x"))
    }

    func testStopFromActiveStateReleasesEverything() {
        let states: [AudioPlaybackState] = [
            .running("A"), .interrupted("A"), .background("A"), .failed("A", reason: "x"),
        ]
        for state in states {
            var machine = AudioPlaybackMachine(state: state)
            XCTAssertEqual(
                machine.handle(.stop),
                [.stopEngine, .closeAudio, .deactivateSession,
                 .emit(state: .idle, reason: "stop")]
            )
            XCTAssertEqual(machine.state, .idle)
            XCTAssertEqual(machine.handle(.stop), [])
        }
    }

    func testExplicitStartAndForegroundCanResumeAfterInterruptionDeclinesResume() {
        for event in [AudioPlaybackEvent.start("A"), .enterForeground] {
            var machine = AudioPlaybackMachine(state: .running("A"))
            _ = machine.handle(.interruptionBegan)
            _ = machine.handle(.interruptionEnded(shouldResume: false))
            let actions = machine.handle(event)
            XCTAssertTrue(actions.contains(.activateSession))
            XCTAssertTrue(actions.contains(.startEngine))
            XCTAssertFalse(actions.contains(.openAudio("A")))
            XCTAssertEqual(machine.state, .running("A"))
        }
    }

    func testInterruptionEndingInBackgroundWaitsForForeground() {
        var machine = AudioPlaybackMachine(state: .running("A"))
        _ = machine.handle(.interruptionBegan)
        _ = machine.handle(.enterBackground)
        XCTAssertEqual(machine.handle(.interruptionEnded(shouldResume: true)), [])
        XCTAssertEqual(machine.state, .background("A"))
        XCTAssertTrue(machine.handle(.enterForeground).contains(.startEngine))
    }

    func testForegroundDuringInterruptionWaitsForInterruptionEnd() {
        var machine = AudioPlaybackMachine(state: .running("A"))
        _ = machine.handle(.enterBackground)
        _ = machine.handle(.interruptionBegan)
        XCTAssertEqual(
            machine.handle(.enterForeground),
            [.emit(state: .interrupted("A"), reason: "interruption")]
        )
        XCTAssertTrue(machine.handle(.interruptionEnded(shouldResume: true)).contains(.startEngine))
    }

    func testStartInBackgroundOnlyOpensHandleUntilForeground() {
        var machine = AudioPlaybackMachine()
        XCTAssertEqual(machine.handle(.enterBackground), [])
        XCTAssertEqual(
            machine.handle(.start("A")),
            [.openAudio("A"), .emit(state: .background("A"), reason: "background")]
        )
        XCTAssertEqual(machine.handle(.configurationChanged), [])
        XCTAssertTrue(machine.handle(.enterForeground).contains(.startEngine))
    }

    func testResetWhilePausedOrIdleOnlyRebuilds() {
        for state in [AudioPlaybackState.idle, .background("A"), .interrupted("A"),
                      .failed("A", reason: "x")] {
            var machine = AudioPlaybackMachine(state: state)
            XCTAssertEqual(machine.handle(.mediaServicesReset), [.rebuildEngine])
            XCTAssertEqual(machine.state, state)
        }
    }

    func testStartRetriesFailedSessionByReopeningItsHandle() {
        var machine = AudioPlaybackMachine(state: .failed("A", reason: "x"))
        let actions = machine.handle(.start("A"))
        XCTAssertTrue(actions.contains(.closeAudio))
        XCTAssertTrue(actions.contains(.openAudio("A")))
        XCTAssertTrue(actions.contains(.startEngine))
        XCTAssertEqual(machine.state, .running("A"))
    }

    func testStartSameRunningSessionOnlyEnsuresEngineIsRunning() {
        var machine = AudioPlaybackMachine(state: .running("A"))
        XCTAssertEqual(machine.handle(.start("A")), [.startEngine])
    }

    func testStopDoesNotForgetTheAppIsInBackground() {
        var machine = AudioPlaybackMachine(state: .background("A"))
        _ = machine.handle(.stop)
        XCTAssertEqual(
            machine.handle(.start("B")),
            [.openAudio("B"), .emit(state: .background("B"), reason: "background")]
        )
    }

    func testStartingDuringAnActiveInterruptionDefersTheEngine() {
        var machine = AudioPlaybackMachine()
        _ = machine.handle(.interruptionBegan)
        XCTAssertEqual(
            machine.handle(.start("A")),
            [.openAudio("A"), .emit(state: .interrupted("A"), reason: "interruption")]
        )
        XCTAssertTrue(machine.handle(.interruptionEnded(shouldResume: true)).contains(.startEngine))
    }
}
