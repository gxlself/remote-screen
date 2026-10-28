package ai.gxlself.remotescreen.logic.audio

/** Platform independent state of the single audio output owned by the app. */
sealed class AudioPlaybackState {
    open val session: String? = null

    data object Idle : AudioPlaybackState()

    data class Running(override val session: String) : AudioPlaybackState()

    data class Interrupted(override val session: String) : AudioPlaybackState()

    data class Background(override val session: String) : AudioPlaybackState()

    data class Failed(override val session: String, val reason: String) : AudioPlaybackState()

    val name: String
        get() = when (this) {
            Idle -> "idle"
            is Running -> "running"
            is Interrupted -> "interrupted"
            is Background -> "background"
            is Failed -> "failed"
        }

    val sessionId: String?
        get() = session

    companion object {
        // Lower-case aliases make the state names read like the Swift contract
        // in call sites while keeping Kotlin's conventional nested types.
        val idle: AudioPlaybackState = Idle
        fun running(session: String): AudioPlaybackState = Running(session)
        fun interrupted(session: String): AudioPlaybackState = Interrupted(session)
        fun background(session: String): AudioPlaybackState = Background(session)
        fun failed(session: String, reason: String): AudioPlaybackState = Failed(session, reason)
    }
}

sealed class AudioPlaybackEvent {
    data class Start(val session: String) : AudioPlaybackEvent()

    data object Stop : AudioPlaybackEvent()

    data object InterruptionBegan : AudioPlaybackEvent()

    data class InterruptionEnded(val shouldResume: Boolean) : AudioPlaybackEvent()

    data object EnterBackground : AudioPlaybackEvent()

    data object EnterForeground : AudioPlaybackEvent()

    data object ConfigurationChanged : AudioPlaybackEvent()

    data object MediaServicesReset : AudioPlaybackEvent()

    data class EngineStartFailed(val reason: String) : AudioPlaybackEvent()

    companion object {
        fun start(session: String): AudioPlaybackEvent = Start(session)
        val stop: AudioPlaybackEvent = Stop
        val interruptionBegan: AudioPlaybackEvent = InterruptionBegan
        fun interruptionEnded(shouldResume: Boolean): AudioPlaybackEvent =
            InterruptionEnded(shouldResume)
        val enterBackground: AudioPlaybackEvent = EnterBackground
        val enterForeground: AudioPlaybackEvent = EnterForeground
        val configurationChanged: AudioPlaybackEvent = ConfigurationChanged
        val mediaServicesReset: AudioPlaybackEvent = MediaServicesReset
        fun engineStartFailed(reason: String): AudioPlaybackEvent = EngineStartFailed(reason)
    }
}

sealed class AudioPlaybackAction {
    data object ActivateSession : AudioPlaybackAction()

    data object DeactivateSession : AudioPlaybackAction()

    data class OpenAudio(val session: String) : AudioPlaybackAction()

    data object CloseAudio : AudioPlaybackAction()

    data object StartEngine : AudioPlaybackAction()

    data object StopEngine : AudioPlaybackAction()

    data object RebuildEngine : AudioPlaybackAction()

    data class Emit(val state: AudioPlaybackState, val reason: String) : AudioPlaybackAction()

    companion object {
        val activateSession: AudioPlaybackAction = ActivateSession
        val deactivateSession: AudioPlaybackAction = DeactivateSession
        fun openAudio(session: String): AudioPlaybackAction = OpenAudio(session)
        val closeAudio: AudioPlaybackAction = CloseAudio
        val startEngine: AudioPlaybackAction = StartEngine
        val stopEngine: AudioPlaybackAction = StopEngine
        val rebuildEngine: AudioPlaybackAction = RebuildEngine
        fun emit(state: AudioPlaybackState, reason: String): AudioPlaybackAction = Emit(state, reason)
    }
}

/** Pure transition logic; platform adapters interpret the returned actions. */
class AudioPlaybackMachine(state: AudioPlaybackState = AudioPlaybackState.Idle) {
    var state: AudioPlaybackState
        private set

    private var inBackground = state is AudioPlaybackState.Background
    private var interruptionActive = false

    init {
        this.state = state
    }

    fun handle(event: AudioPlaybackEvent): List<AudioPlaybackAction> = when (event) {
        is AudioPlaybackEvent.Start -> start(event.session)
        AudioPlaybackEvent.Stop -> stop()
        AudioPlaybackEvent.InterruptionBegan -> interruptionBegan()
        is AudioPlaybackEvent.InterruptionEnded -> interruptionEnded(event.shouldResume)
        AudioPlaybackEvent.EnterBackground -> enterBackground()
        AudioPlaybackEvent.EnterForeground -> enterForeground()
        AudioPlaybackEvent.ConfigurationChanged ->
            if (state is AudioPlaybackState.Running) {
                listOf(AudioPlaybackAction.StartEngine)
            } else {
                emptyList()
            }
        AudioPlaybackEvent.MediaServicesReset -> when (state) {
            is AudioPlaybackState.Running -> listOf(
                AudioPlaybackAction.RebuildEngine,
                AudioPlaybackAction.ActivateSession,
                AudioPlaybackAction.StartEngine,
            )
            AudioPlaybackState.Idle,
            is AudioPlaybackState.Interrupted,
            is AudioPlaybackState.Background,
            is AudioPlaybackState.Failed,
            -> listOf(AudioPlaybackAction.RebuildEngine)
        }
        is AudioPlaybackEvent.EngineStartFailed -> {
            val session = state.sessionId
            if (session == null) {
                emptyList()
            } else {
                state = AudioPlaybackState.Failed(session, event.reason)
                listOf(
                    AudioPlaybackAction.StopEngine,
                    AudioPlaybackAction.CloseAudio,
                    AudioPlaybackAction.DeactivateSession,
                    AudioPlaybackAction.Emit(state, event.reason),
                )
            }
        }
    }

    private fun start(session: String): List<AudioPlaybackAction> {
        if (session.isEmpty()) return emptyList()

        val previous = state
        var needsOpen = previous.sessionId != session
        if (previous is AudioPlaybackState.Failed) needsOpen = true
        val actions = mutableListOf<AudioPlaybackAction>()
        if (needsOpen && previous != AudioPlaybackState.Idle) {
            actions += AudioPlaybackAction.StopEngine
            actions += AudioPlaybackAction.CloseAudio
        }

        state = when {
            inBackground -> AudioPlaybackState.Background(session)
            interruptionActive -> AudioPlaybackState.Interrupted(session)
            else -> AudioPlaybackState.Running(session)
        }
        if (!inBackground && !interruptionActive && previous != state) {
            actions += AudioPlaybackAction.ActivateSession
        }
        if (needsOpen) actions += AudioPlaybackAction.OpenAudio(session)
        if (state is AudioPlaybackState.Running) actions += AudioPlaybackAction.StartEngine
        if (previous != state) {
            val reason = when {
                inBackground -> "background"
                interruptionActive -> "interruption"
                else -> "start"
            }
            actions += AudioPlaybackAction.Emit(state, reason)
        }
        return actions
    }

    private fun stop(): List<AudioPlaybackAction> {
        if (state == AudioPlaybackState.Idle) return emptyList()
        state = AudioPlaybackState.Idle
        return listOf(
            AudioPlaybackAction.StopEngine,
            AudioPlaybackAction.CloseAudio,
            AudioPlaybackAction.DeactivateSession,
            AudioPlaybackAction.Emit(state, "stop"),
        )
    }

    private fun interruptionBegan(): List<AudioPlaybackAction> {
        interruptionActive = true
        val session = (state as? AudioPlaybackState.Running)?.session ?: return emptyList()
        state = AudioPlaybackState.Interrupted(session)
        return listOf(
            AudioPlaybackAction.StopEngine,
            AudioPlaybackAction.Emit(state, "interruption"),
        )
    }

    private fun interruptionEnded(shouldResume: Boolean): List<AudioPlaybackAction> {
        interruptionActive = false
        if (inBackground || !shouldResume) return emptyList()
        val session = (state as? AudioPlaybackState.Interrupted)?.session ?: return emptyList()
        state = AudioPlaybackState.Running(session)
        return listOf(
            AudioPlaybackAction.ActivateSession,
            AudioPlaybackAction.StartEngine,
            AudioPlaybackAction.Emit(state, "interruptionEnded"),
        )
    }

    private fun enterBackground(): List<AudioPlaybackAction> {
        inBackground = true
        val session = when (val current = state) {
            is AudioPlaybackState.Running -> current.session
            is AudioPlaybackState.Interrupted -> current.session
            AudioPlaybackState.Idle,
            is AudioPlaybackState.Background,
            is AudioPlaybackState.Failed,
            -> return emptyList()
        }
        state = AudioPlaybackState.Background(session)
        return listOf(
            AudioPlaybackAction.StopEngine,
            AudioPlaybackAction.DeactivateSession,
            AudioPlaybackAction.Emit(state, "background"),
        )
    }

    private fun enterForeground(): List<AudioPlaybackAction> {
        inBackground = false
        val session = when (val current = state) {
            is AudioPlaybackState.Background -> current.session
            is AudioPlaybackState.Interrupted -> current.session
            AudioPlaybackState.Idle,
            is AudioPlaybackState.Running,
            is AudioPlaybackState.Failed,
            -> return emptyList()
        }
        if (interruptionActive) {
            val next = AudioPlaybackState.Interrupted(session)
            if (state == next) return emptyList()
            state = next
            return listOf(AudioPlaybackAction.Emit(state, "interruption"))
        }
        state = AudioPlaybackState.Running(session)
        return listOf(
            AudioPlaybackAction.ActivateSession,
            AudioPlaybackAction.StartEngine,
            AudioPlaybackAction.Emit(state, "foreground"),
        )
    }
}
