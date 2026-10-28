package ai.gxlself.remotescreen.logic.audio

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioTests {
    @Test
    fun startFromIdleActivatesOpensAndStarts() {
        val machine = AudioPlaybackMachine()
        assertEquals(
            listOf(
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.OpenAudio("A"),
                AudioPlaybackAction.StartEngine,
                AudioPlaybackAction.Emit(AudioPlaybackState.Running("A"), "start"),
            ),
            machine.handle(AudioPlaybackEvent.Start("A")),
        )
        assertEquals(AudioPlaybackState.Running("A"), machine.state)
    }

    @Test
    fun startingAnotherSessionClosesTheCurrentHandle() {
        val machine = AudioPlaybackMachine()
        machine.handle(AudioPlaybackEvent.Start("A"))
        assertEquals(
            listOf(
                AudioPlaybackAction.StopEngine,
                AudioPlaybackAction.CloseAudio,
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.OpenAudio("B"),
                AudioPlaybackAction.StartEngine,
                AudioPlaybackAction.Emit(AudioPlaybackState.Running("B"), "start"),
            ),
            machine.handle(AudioPlaybackEvent.Start("B")),
        )
        assertEquals(AudioPlaybackState.Running("B"), machine.state)
    }

    @Test
    fun interruptionStopsAndResumes() {
        val machine = AudioPlaybackMachine()
        machine.handle(AudioPlaybackEvent.Start("A"))
        assertEquals(
            listOf(
                AudioPlaybackAction.StopEngine,
                AudioPlaybackAction.Emit(AudioPlaybackState.Interrupted("A"), "interruption"),
            ),
            machine.handle(AudioPlaybackEvent.InterruptionBegan),
        )
        assertEquals(
            listOf(
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.StartEngine,
                AudioPlaybackAction.Emit(AudioPlaybackState.Running("A"), "interruptionEnded"),
            ),
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = true)),
        )
    }

    @Test
    fun interruptionWithoutResumeStaysInterrupted() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Interrupted("A"))
        assertEquals(
            emptyList<AudioPlaybackAction>(),
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = false)),
        )
        assertEquals(AudioPlaybackState.Interrupted("A"), machine.state)
    }

    @Test
    fun backgroundStopsAndForegroundResumes() {
        val machine = AudioPlaybackMachine()
        machine.handle(AudioPlaybackEvent.Start("A"))
        assertEquals(
            listOf(
                AudioPlaybackAction.StopEngine,
                AudioPlaybackAction.DeactivateSession,
                AudioPlaybackAction.Emit(AudioPlaybackState.Background("A"), "background"),
            ),
            machine.handle(AudioPlaybackEvent.EnterBackground),
        )
        assertEquals(
            listOf(
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.StartEngine,
                AudioPlaybackAction.Emit(AudioPlaybackState.Running("A"), "foreground"),
            ),
            machine.handle(AudioPlaybackEvent.EnterForeground),
        )
    }

    @Test
    fun configurationOnlyRestartsRunningEngine() {
        val machine = AudioPlaybackMachine()
        assertEquals(emptyList<AudioPlaybackAction>(), machine.handle(AudioPlaybackEvent.ConfigurationChanged))
        machine.handle(AudioPlaybackEvent.Start("A"))
        assertEquals(
            listOf(AudioPlaybackAction.StartEngine),
            machine.handle(AudioPlaybackEvent.ConfigurationChanged),
        )
    }

    @Test
    fun mediaServicesResetRebuildsRunningEngine() {
        val machine = AudioPlaybackMachine()
        machine.handle(AudioPlaybackEvent.Start("A"))
        assertEquals(
            listOf(
                AudioPlaybackAction.RebuildEngine,
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.StartEngine,
            ),
            machine.handle(AudioPlaybackEvent.MediaServicesReset),
        )
    }

    @Test
    fun engineStartFailureEmitsFailedState() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Running("A"))
        assertEquals(
            listOf(
                AudioPlaybackAction.StopEngine,
                AudioPlaybackAction.CloseAudio,
                AudioPlaybackAction.DeactivateSession,
                AudioPlaybackAction.Emit(AudioPlaybackState.Failed("A", "x"), "x"),
            ),
            machine.handle(AudioPlaybackEvent.EngineStartFailed("x")),
        )
        assertEquals(AudioPlaybackState.Failed("A", "x"), machine.state)
    }

    @Test
    fun stopFromActiveStateReleasesEverything() {
        val states = listOf<AudioPlaybackState>(
            AudioPlaybackState.Running("A"),
            AudioPlaybackState.Interrupted("A"),
            AudioPlaybackState.Background("A"),
            AudioPlaybackState.Failed("A", "x"),
        )
        states.forEach { state ->
            val machine = AudioPlaybackMachine(state)
            assertEquals(
                listOf(
                    AudioPlaybackAction.StopEngine,
                    AudioPlaybackAction.CloseAudio,
                    AudioPlaybackAction.DeactivateSession,
                    AudioPlaybackAction.Emit(AudioPlaybackState.Idle, "stop"),
                ),
                machine.handle(AudioPlaybackEvent.Stop),
            )
            assertEquals(AudioPlaybackState.Idle, machine.state)
            assertEquals(emptyList<AudioPlaybackAction>(), machine.handle(AudioPlaybackEvent.Stop))
        }
    }

    @Test
    fun explicitStartAndForegroundCanResumeAfterInterruptionDeclinesResume() {
        listOf<AudioPlaybackEvent>(
            AudioPlaybackEvent.Start("A"),
            AudioPlaybackEvent.EnterForeground,
        ).forEach { event ->
            val machine = AudioPlaybackMachine(AudioPlaybackState.Running("A"))
            machine.handle(AudioPlaybackEvent.InterruptionBegan)
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = false))
            val actions = machine.handle(event)
            assertTrue(actions.contains(AudioPlaybackAction.ActivateSession))
            assertTrue(actions.contains(AudioPlaybackAction.StartEngine))
            assertFalse(actions.contains(AudioPlaybackAction.OpenAudio("A")))
            assertEquals(AudioPlaybackState.Running("A"), machine.state)
        }
    }

    @Test
    fun interruptionEndingInBackgroundWaitsForForeground() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Running("A"))
        machine.handle(AudioPlaybackEvent.InterruptionBegan)
        machine.handle(AudioPlaybackEvent.EnterBackground)
        assertEquals(
            emptyList<AudioPlaybackAction>(),
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = true)),
        )
        assertEquals(AudioPlaybackState.Background("A"), machine.state)
        assertTrue(machine.handle(AudioPlaybackEvent.EnterForeground).contains(AudioPlaybackAction.StartEngine))
    }

    @Test
    fun foregroundDuringInterruptionWaitsForInterruptionEnd() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Running("A"))
        machine.handle(AudioPlaybackEvent.EnterBackground)
        machine.handle(AudioPlaybackEvent.InterruptionBegan)
        assertEquals(
            listOf(AudioPlaybackAction.Emit(AudioPlaybackState.Interrupted("A"), "interruption")),
            machine.handle(AudioPlaybackEvent.EnterForeground),
        )
        assertTrue(
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = true))
                .contains(AudioPlaybackAction.StartEngine),
        )
    }

    @Test
    fun startInBackgroundOnlyOpensHandleUntilForeground() {
        val machine = AudioPlaybackMachine()
        assertEquals(emptyList<AudioPlaybackAction>(), machine.handle(AudioPlaybackEvent.EnterBackground))
        assertEquals(
            listOf(
                AudioPlaybackAction.OpenAudio("A"),
                AudioPlaybackAction.Emit(AudioPlaybackState.Background("A"), "background"),
            ),
            machine.handle(AudioPlaybackEvent.Start("A")),
        )
        assertEquals(emptyList<AudioPlaybackAction>(), machine.handle(AudioPlaybackEvent.ConfigurationChanged))
        assertTrue(machine.handle(AudioPlaybackEvent.EnterForeground).contains(AudioPlaybackAction.StartEngine))
    }

    @Test
    fun resetWhilePausedOrIdleOnlyRebuilds() {
        listOf<AudioPlaybackState>(
            AudioPlaybackState.Idle,
            AudioPlaybackState.Background("A"),
            AudioPlaybackState.Interrupted("A"),
            AudioPlaybackState.Failed("A", "x"),
        ).forEach { state ->
            val machine = AudioPlaybackMachine(state)
            assertEquals(
                listOf(AudioPlaybackAction.RebuildEngine),
                machine.handle(AudioPlaybackEvent.MediaServicesReset),
            )
            assertEquals(state, machine.state)
        }
    }

    @Test
    fun startRetriesFailedSessionByReopeningItsHandle() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Failed("A", "x"))
        val actions = machine.handle(AudioPlaybackEvent.Start("A"))
        assertTrue(actions.contains(AudioPlaybackAction.CloseAudio))
        assertTrue(actions.contains(AudioPlaybackAction.OpenAudio("A")))
        assertTrue(actions.contains(AudioPlaybackAction.StartEngine))
        assertEquals(AudioPlaybackState.Running("A"), machine.state)
    }

    @Test
    fun startSameRunningSessionOnlyEnsuresEngineIsRunning() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Running("A"))
        assertEquals(
            listOf(AudioPlaybackAction.StartEngine),
            machine.handle(AudioPlaybackEvent.Start("A")),
        )
    }

    @Test
    fun stopDoesNotForgetTheAppIsInBackground() {
        val machine = AudioPlaybackMachine(AudioPlaybackState.Background("A"))
        machine.handle(AudioPlaybackEvent.Stop)
        assertEquals(
            listOf(
                AudioPlaybackAction.OpenAudio("B"),
                AudioPlaybackAction.Emit(AudioPlaybackState.Background("B"), "background"),
            ),
            machine.handle(AudioPlaybackEvent.Start("B")),
        )
    }

    @Test
    fun startingDuringAnActiveInterruptionDefersTheEngine() {
        val machine = AudioPlaybackMachine()
        machine.handle(AudioPlaybackEvent.InterruptionBegan)
        assertEquals(
            listOf(
                AudioPlaybackAction.OpenAudio("A"),
                AudioPlaybackAction.Emit(AudioPlaybackState.Interrupted("A"), "interruption"),
            ),
            machine.handle(AudioPlaybackEvent.Start("A")),
        )
        assertTrue(
            machine.handle(AudioPlaybackEvent.InterruptionEnded(shouldResume = true))
                .contains(AudioPlaybackAction.StartEngine),
        )
    }
}
