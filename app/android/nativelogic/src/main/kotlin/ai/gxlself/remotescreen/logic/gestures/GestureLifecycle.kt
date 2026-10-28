package ai.gxlself.remotescreen.logic.gestures

/** Lifecycle states for resources owned by an Android session gesture layer. */
class GestureLifecycle(initialState: State = State.Paused) {
  enum class State {
    Active,
    Paused,
    Closed,
  }

  enum class Action {
    ReleaseInputs,
    CancelGestures,
    RecycleVelocityTracker,
    ClearEdgeExclusion,
    RegisterHooks,
    UnregisterHooks,
    BindSession,
    RestoreEdgeExclusion,
    CloseResources,
  }

  /** The side effects requested by one lifecycle event. */
  data class Actions(
    val releaseInputs: Boolean = false,
    val cancelGestures: Boolean = false,
    val recycleVelocityTracker: Boolean = false,
    val clearEdgeExclusion: Boolean = false,
    val registerHooks: Boolean = false,
    val unregisterHooks: Boolean = false,
    val bindSession: Boolean = false,
    val restoreEdgeExclusion: Boolean = false,
    val closeResources: Boolean = false,
  ) {
    val isEmpty: Boolean
      get() = !releaseInputs &&
        !cancelGestures &&
        !recycleVelocityTracker &&
        !clearEdgeExclusion &&
        !registerHooks &&
        !unregisterHooks &&
        !bindSession &&
        !restoreEdgeExclusion &&
        !closeResources

    fun contains(action: Action): Boolean = when (action) {
      Action.ReleaseInputs -> releaseInputs
      Action.CancelGestures -> cancelGestures
      Action.RecycleVelocityTracker -> recycleVelocityTracker
      Action.ClearEdgeExclusion -> clearEdgeExclusion
      Action.RegisterHooks -> registerHooks
      Action.UnregisterHooks -> unregisterHooks
      Action.BindSession -> bindSession
      Action.RestoreEdgeExclusion -> restoreEdgeExclusion
      Action.CloseResources -> closeResources
    }

    fun asSet(): Set<Action> = buildSet {
      if (releaseInputs) add(Action.ReleaseInputs)
      if (cancelGestures) add(Action.CancelGestures)
      if (recycleVelocityTracker) add(Action.RecycleVelocityTracker)
      if (clearEdgeExclusion) add(Action.ClearEdgeExclusion)
      if (registerHooks) add(Action.RegisterHooks)
      if (unregisterHooks) add(Action.UnregisterHooks)
      if (bindSession) add(Action.BindSession)
      if (restoreEdgeExclusion) add(Action.RestoreEdgeExclusion)
      if (closeResources) add(Action.CloseResources)
    }
  }

  var state: State = initialState
    private set

  /** Exposed for tests and adapters that apply the returned actions. */
  var hooksRegistered: Boolean = initialState == State.Active
    private set

  init {
    require(initialState != State.Closed || !hooksRegistered) {
      "A closed gesture lifecycle cannot have registered hooks"
    }
  }

  /** Moves a paused layer into the window and requests hook registration. */
  fun windowAttached(): Actions {
    if (state != State.Paused) return Actions()
    state = State.Active
    if (hooksRegistered) return Actions()
    hooksRegistered = true
    return Actions(
      registerHooks = true,
      bindSession = true,
      restoreEdgeExclusion = true,
    )
  }

  /** Pauses an active layer while keeping reusable input resources alive. */
  fun windowDetached(): Actions {
    if (state != State.Active) return Actions()
    state = State.Paused
    val unregister = hooksRegistered
    hooksRegistered = false
    return Actions(
      releaseInputs = true,
      cancelGestures = true,
      recycleVelocityTracker = true,
      clearEdgeExclusion = true,
      unregisterHooks = unregister,
    )
  }

  /** Permanently closes the layer. Closing an active layer includes pausing it first. */
  fun drop(): Actions {
    if (state == State.Closed) return Actions()
    val wasActive = state == State.Active
    val unregister = hooksRegistered
    state = State.Closed
    hooksRegistered = false
    return Actions(
      releaseInputs = wasActive,
      cancelGestures = wasActive,
      recycleVelocityTracker = wasActive,
      clearEdgeExclusion = wasActive,
      unregisterHooks = unregister,
      closeResources = true,
    )
  }
}

typealias GestureLifecycleState = GestureLifecycle.State
typealias GestureLifecycleAction = GestureLifecycle.Action
typealias GestureLifecycleActions = GestureLifecycle.Actions
