import AVFoundation
import UIKit
import VideoToolbox

@objc(RemoteSessionSurface)
final class RemoteSessionSurface: UIView, SessionCursorSink {
    // Keep the video in a transformable content layer. The view itself remains
    // untransformed so UIKit gesture coordinates stay stable; the cursor layer
    // receives the same viewport mapping from SessionCursorController.
    private let contentLayer = CALayer()
    private let backdropLayer = CALayer()
    private let displayLayer = AVSampleBufferDisplayLayer()
    private lazy var zoomController = SessionZoomController(layer: contentLayer)
    private lazy var frameRateController = SessionFrameRateController(surface: self)
    private let magnifierLayer = SessionMagnifierLayer()
    /// The last picture of this session while its first frame is on the way.
    private let snapshotLayer = CALayer()
    private var spaceSwipe: SpaceSwipeController?
    private var firstFrameShown = false
    let pencilHoverLayer = SessionPencilHoverLayer()

    @objc var onGesture: ((NSDictionary) -> Void)?
    @objc var onPointerLock: ((NSDictionary) -> Void)?
    @objc var onViewport: ((NSDictionary) -> Void)?
    @objc var onFirstFrame: ((NSDictionary) -> Void)?
    @objc var onVideoStats: ((NSDictionary) -> Void)?
    /// a three-finger swipe began / ended ({"phase"}).
    @objc var onSpaceSwipe: ((NSDictionary) -> Void)?
    /// the slide to another computer finished ({"sessionId"}).
    @objc var onSpaceSwitch: ((NSDictionary) -> Void)?
    /// Show this session's last picture until its first frame (a switch).
    @objc var showSnapshot: Bool = false { didSet { if showSnapshot != oldValue { updateSnapshotPlaceholder() } } }

    @objc var mode: String = "trackpad" { didSet { if mode != oldValue { modeDidChange() } } }
    @objc var keyboardInset: CGFloat = 0 { didSet { keyboardInsetDidChange() } }
    @objc var reduceMotion: Bool = false { didSet { reduceMotionDidChange() } }
    @objc var toolbarCollapsed: Bool = false // Android-only behavior.
    @objc var cursorSpeed: CGFloat {
        get { prefs.cursorSpeed }
        set { prefs = SessionGesturePrefs(cursorSpeed: newValue, naturalScroll: prefs.naturalScroll, haptics: prefs.haptics) }
    }
    @objc var naturalScroll: Bool {
        get { prefs.naturalScroll }
        set { prefs = SessionGesturePrefs(cursorSpeed: prefs.cursorSpeed, naturalScroll: newValue, haptics: prefs.haptics) }
    }
    @objc var haptics: Bool {
        get { prefs.haptics }
        set { prefs = SessionGesturePrefs(cursorSpeed: prefs.cursorSpeed, naturalScroll: prefs.naturalScroll, haptics: newValue) }
    }
    private(set) var prefs = SessionGesturePrefs() {
        didSet {
            guard prefs != oldValue else { return }
            RsLog.marker("SESSVIEW-PREFS", ["cursorSpeed": Double(prefs.cursorSpeed),
                                            "naturalScroll": prefs.naturalScroll, "haptics": prefs.haptics])
        }
    }
    private(set) var videoContentRect: CGRect = .zero
    private(set) var sessionId: UInt64?
    var sessionInputReady: Bool { inputAttached && sessionId != nil }
    var pencilActive = false
    var pointerVisible: Bool {
        guard pointerApplicationActive, window != nil else { return false }
        var ancestor: UIView? = self
        while let view = ancestor {
            if view.isHidden || view.alpha <= 0 { return false }
            ancestor = view.superview
        }
        return true
    }
    var cursorSink: SessionCursorSink = SessionNullCursorSink()
    var viewport: SessionViewport { zoomController.viewport }

    private let attachment = SessionAttachment()
    private var pipeline: SessionVideoPipeline?
    /// M3 V3：画中画用的源图层和视频管线（只读，主线程用）。
    var pipDisplayLayer: AVSampleBufferDisplayLayer { displayLayer }
    var pipPipeline: SessionVideoPipeline? { pipeline }
    private var sessionValue = ""
    private var videoSize: CGSize = .zero
    private var decodedVideoSize: CGSize = .zero
    private var videoSource = CGRect(x: 0, y: 0, width: 1, height: 1)
    private var backdropImage: CGImage?
    private var viewportCropAllowed = true
    private var inputAttached = false
    private let inputSender = SessionInputSender()
    private let cursorController = SessionCursorController()
    private var trackpadGestures: TrackpadGestureController?
    private var touchGestures: TouchGestureController?
    private var pencilGestures: PencilGestureController?
    private var pointerGestures: PointerGestureController?
    private var pointerApplicationActive = UIApplication.shared.applicationState == .active
    private var pointerMotion = PointerMotion()
    private var pointerEscape = PointerEdgeEscape()
    private var pointerScroll = PointerScrollSynth()
    private var pointerClicks = TrackpadClickCounter()
    private var pointerButtons: [Int32: UInt8] = [:]
    private var pointerScrollWork: DispatchWorkItem?
    private var pointerScrollGeneration: UInt64 = 0
    private var pointerScrollMomentum = TrackpadScrollMomentum()
    private var pointerMomentumLink: CADisplayLink?
    private var lastPointerMomentumTime: CFTimeInterval?
    private lazy var pointerMomentumTarget: PointerMomentumTarget = {
        let target = PointerMomentumTarget()
        target.surface = self
        return target
    }()
    private var lastPointerMoveLog: TimeInterval?
    private static let pointerMoveLogInterval: TimeInterval = 1
    private var lastLockedScroll: TimeInterval?
    private static let lockedScrollQuietAfter: TimeInterval = 0.5
    private var lastPointerLockState: PointerLockState?

    private struct PointerLockState: Equatable {
        let mouse: Bool
        let locked: Bool
        let reason: PointerLockReason
    }
    private var savedViewportForKeyboard: SessionViewport?
    private var lastLayoutSize: CGSize?

    override var isHidden: Bool {
        didSet {
            frameRateController.updateVisibility()
            PointerLockCoordinator.shared.surfaceDidChange(self)
            SessionPipCoordinator.shared.surfaceDidChange(self)
        }
    }
    override var alpha: CGFloat {
        didSet {
            frameRateController.updateVisibility()
            PointerLockCoordinator.shared.surfaceDidChange(self)
            SessionPipCoordinator.shared.surfaceDidChange(self)
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .black
        // Fabric's outer view is the single accessibility element.
        isAccessibilityElement = false
        accessibilityElementsHidden = true
        clipsToBounds = true
        contentLayer.anchorPoint = .zero
        contentLayer.position = .zero
        contentLayer.bounds = CGRect(origin: .zero, size: bounds.size)
        contentLayer.masksToBounds = false
        layer.addSublayer(contentLayer)
        backdropLayer.contentsGravity = .resize
        contentLayer.addSublayer(backdropLayer)
        displayLayer.videoGravity = .resizeAspect
        contentLayer.addSublayer(displayLayer)
        snapshotLayer.contentsGravity = .resizeAspect
        snapshotLayer.backgroundColor = UIColor.black.cgColor
        snapshotLayer.isHidden = true
        layer.addSublayer(snapshotLayer)
        cursorSink = cursorController
        // Keep the cursor as a sibling so the overlay is not clipped by the
        // video layer; syncCursorLayer maps its tip with the content transform.
        layer.addSublayer(cursorController.layer)
        layer.addSublayer(magnifierLayer)
        layer.addSublayer(pencilHoverLayer)

        zoomController.onViewport = { [weak self] viewport in
            guard let self else { return }
            self.cursorController.update(viewport: viewport, videoSize: self.videoSize)
            self.syncCursorLayer()
            self.refreshMagnifier()
            self.onViewport?(["zoom": viewport.zoom, "x": viewport.offset.x, "y": viewport.offset.y])
            self.reportViewport()
        }
        let trackpad = TrackpadGestureController(surface: self, input: inputSender, cursor: cursorController)
        trackpadGestures = trackpad
        let touch = TouchGestureController(surface: self, input: inputSender, cursor: cursorController)
        touchGestures = touch
        let pencil = PencilGestureController(surface: self, input: inputSender, cursor: cursorController)
        pencilGestures = pencil
        let pointer = PointerGestureController(surface: self, input: inputSender, cursor: cursorController)
        pointerGestures = pointer
        trackpad.setSuspended(UIApplication.shared.applicationState != .active)
        touch.setSuspended(UIApplication.shared.applicationState != .active)
        pencil.setSuspended(UIApplication.shared.applicationState != .active)
        pointer.setSuspended(UIApplication.shared.applicationState != .active)
        trackpad.install()
        pencil.install()
        pointer.install()
        let spaces = SpaceSwipeController(surface: self)
        spaces.onSwipe = { [weak self] phase in self?.onSpaceSwipe?(["phase": phase]) }
        spaces.onSwitch = { [weak self] target in self?.onSpaceSwitch?(["sessionId": target]) }
        spaces.setSuspended(UIApplication.shared.applicationState != .active)
        spaceSwipe = spaces
        NotificationCenter.default.addObserver(self, selector: #selector(snapshotDidChange(_:)),
                                               name: SessionSnapshotStore.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(willResignActive),
                                               name: UIApplication.willResignActiveNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(didBecomeActive),
                                               name: UIApplication.didBecomeActiveNotification, object: nil)
        // pointer, gestures and input follow this surface's own window.
        NotificationCenter.default.addObserver(self, selector: #selector(sceneWillDeactivate(_:)),
                                               name: UIScene.willDeactivateNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(sceneDidActivate(_:)),
                                               name: UIScene.didActivateNotification, object: nil)
    }

    required init?(coder: NSCoder) { return nil }

    /// The session shown, as JS names it; nil when detached.
    var sessionKey: String? { sessionValue.isEmpty ? nil : sessionValue }

    /// The swipe recognizer is not a viewport gesture: skip the zoom observer.
    func installSpaceRecognizer(_ recognizer: UIGestureRecognizer) {
        super.addGestureRecognizer(recognizer)
    }

    /// the open computers in order (ids and names side by side).
    @objc(setSpacesWithIds:names:)
    func setSpaces(ids: [String], names: [String]) {
        spaceSwipe?.setSpaces(ids: ids, names: names)
        // A session no longer in the list has ended: drop its picture.
        SessionSnapshotStore.shared.retain(only: ids)
    }

    @objc(animateSpaceTo:direction:)
    func animateSpace(to target: String, direction: String) {
        RsLog.marker("SESSVIEW", ["command": "animateSpace", "direction": direction])
        guard let direction = SpaceSwipeDirection(rawValue: direction) else {
            onSpaceSwitch?(["sessionId": target])
            return
        }
        spaceSwipe?.animate(to: target, direction: direction)
    }

    /// A swipe took over the touch: release anything sent to the remote.
    func cancelInputForSpaceSwipe() {
        cancelPointerInput()
        pencilGestures?.cancel()
        trackpadGestures?.cancel()
        touchGestures?.cancel()
        magnifierLayer.show(false)
    }

    /// Keeps the current picture for swiping back to this session later.
    func captureSnapshot() {
        guard let key = sessionKey, let pipeline else { return }
        let limit = SessionSnapshotStore.screenLimit(window?.screen)
        if ViewportGeometry.isFull(videoSource), let buffer = pipeline.latestPixelBuffer() {
            SessionSnapshotStore.shared.store(buffer, for: key, limit: limit)
        } else if let backdropImage {
            // Zoomed in with a cropped stream: the whole-screen backdrop.
            SessionSnapshotStore.shared.store(backdropImage, for: key, limit: limit)
        }
    }

    private func updateSnapshotPlaceholder() {
        var image: CGImage?
        if showSnapshot, !firstFrameShown, let key = sessionKey {
            image = SessionSnapshotStore.shared.image(for: key)
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        snapshotLayer.contents = image
        snapshotLayer.isHidden = image == nil
        CATransaction.commit()
    }

    @objc private func snapshotDidChange(_ notification: Notification) {
        guard showSnapshot, !firstFrameShown,
              notification.userInfo?["sessionId"] as? String == sessionKey else { return }
        updateSnapshotPlaceholder()
    }

    // Observe existing recognizers without changing their delegates or input
    // routing. This also covers recognizers reinstalled after a mode change.
    override func addGestureRecognizer(_ gestureRecognizer: UIGestureRecognizer) {
        super.addGestureRecognizer(gestureRecognizer)
        zoomController.observeGesture(gestureRecognizer)
    }

    override func removeGestureRecognizer(_ gestureRecognizer: UIGestureRecognizer) {
        zoomController.stopObservingGesture(gestureRecognizer)
        super.removeGestureRecognizer(gestureRecognizer)
    }

    @objc func setViewportCropAllowed(_ allowed: Bool) {
        guard viewportCropAllowed != allowed else { return }
        viewportCropAllowed = allowed
        reportViewport()
    }

    private func reportViewport() {
        guard let sessionId, !videoContentRect.isEmpty else { return }
        let viewport = zoomController.reportingViewport
        let visible = ViewportGeometry.visibleRect(viewport: viewport, viewSize: bounds.size,
                                                   keyboardInset: keyboardInset)
        let pixels = ViewportGeometry.viewPixels(visible: visible, viewport: viewport,
                                                  screenScale: window?.screen.scale ?? traitCollection.displayScale)
        let flags = (zoomController.isInteracting ? UInt32(RS_VIEWPORT_ACTIVE) : 0)
            | (viewportCropAllowed ? UInt32(RS_VIEWPORT_CROP_OK) : 0)
        rs_session_set_viewport(sessionId, Float(visible.minX), Float(visible.minY),
                                Float(visible.width), Float(visible.height), pixels.width, pixels.height, flags)
    }

    @objc(setSessionId:)
    func setSessionId(_ value: String) {
        guard sessionValue != value else { return }
        detach()
        sessionValue = value
        sessionId = SessionIdParse.parse(value)
        guard !value.isEmpty else { return }
        let pipeline = SessionVideoPipeline(layer: displayLayer)
        self.pipeline = pipeline
        magnifierLayer.setPipeline(pipeline)
        pipeline.onFirstFrame = { [weak self, weak pipeline] epoch, width, height in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            self.firstFrameShown = true
            self.updateSnapshotPlaceholder()
            self.markSessionAttached()
            self.frameRateController.setStreaming(true)
            self.onFirstFrame?(["epoch": Int(epoch), "width": width, "height": height])
        }
        pipeline.onVideoSize = { [weak self, weak pipeline] size in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            self.decodedVideoSize = size
            self.videoSize = ViewportGeometry.logicalVideoSize(decoded: size, src: self.videoSource)
            self.updateVideoContentRect(force: true)
        }
        pipeline.onEpochGeometry = { [weak self, weak pipeline] geometry, sample in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            self.presentEpochGeometry(geometry, sample: sample, pipeline: pipeline)
        }
        pipeline.onVideoStats = { [weak self, weak pipeline] stats in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            self.onVideoStats?(stats)
        }
        pipeline.onCursor = { [weak self, weak pipeline] event in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            self.consume(event)
        }
        pipeline.onClosed = { [weak self, weak pipeline] in
            guard let self, let pipeline, self.pipeline === pipeline else { return }
            // Closed by Core: no picture to keep for this session.
            SessionSnapshotStore.shared.remove(value)
            self.detach(capture: false)
        }
        attachment.attach(value, pipeline: pipeline)
        updateSnapshotPlaceholder()
    }

    @objc func detach() {
        detach(capture: true)
    }

    private func detach(capture: Bool) {
        // Leaving this session (folded away, switched, paused): keep its last
        // picture for the swipe and for the page that shows it next.
        if capture { captureSnapshot() }
        firstFrameShown = false
        frameRateController.setStreaming(false)
        cancelPointerInput()
        pencilGestures?.cancel()
        inputAttached = false
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
        trackpadGestures?.cancel()
        touchGestures?.cancel()
        inputSender.setSession(nil)
        attachment.detach()
        pipeline?.stop()
        pipeline = nil
        magnifierLayer.setPipeline(nil)
        magnifierLayer.show(false)
        pencilHoverLayer.hide()
        pencilActive = false
        sessionId = nil
        sessionValue = ""
        cursorController.reset()
        videoSize = .zero
        decodedVideoSize = .zero
        videoSource = CGRect(x: 0, y: 0, width: 1, height: 1)
        clearBackdrop()
        savedViewportForKeyboard = nil
        zoomController.reset()
        updateVideoContentRect()
        updateSnapshotPlaceholder()
    }

    @objc func reset() {
        // Called after detach by prepareForRecycle.
        SampleBufferTarget(layer: displayLayer).flushRemovingDisplayedImage()
        mode = "trackpad"
        keyboardInset = 0
        reduceMotion = false
        toolbarCollapsed = false
        prefs = SessionGesturePrefs()
        videoSize = .zero
        decodedVideoSize = .zero
        videoSource = CGRect(x: 0, y: 0, width: 1, height: 1)
        viewportCropAllowed = true
        clearBackdrop()
        videoContentRect = .zero
        zoomController.reset()
        magnifierLayer.show(false)
        pencilHoverLayer.hide()
        pencilActive = false
        cursorSink = cursorController
        showSnapshot = false
        spaceSwipe?.reset()
        spaceSwipe?.setSpaces(ids: [], names: [])
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        updateVideoContentRect()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        snapshotLayer.frame = bounds
        CATransaction.commit()
        spaceSwipe?.layout(bounds: bounds)
        if lastLayoutSize != bounds.size {
            lastLayoutSize = bounds.size
            NSLog("%@", "RS-SESSVIEW {\"layout\":{\"w\":\(Double(bounds.width)),\"h\":\(Double(bounds.height))}}")
        }
        // Window dimensions can change without a canvas size change (Stage
        // Manager, divider drags); the coordinator deduplicates state events.
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
        frameRateController.updateVisibility()
        reportViewport()
    }

    /// The decode queue pauses frame submission until this main-thread
    /// transaction has placed the first sample in its matching geometry.
    private func presentEpochGeometry(_ geometry: EpochGeometry, sample: CMSampleBuffer,
                                      pipeline: SessionVideoPipeline) {
        precondition(Thread.isMainThread)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if ViewportGeometry.isFull(videoSource), !ViewportGeometry.isFull(geometry.src),
           backdropImage == nil, let buffer = pipeline.latestPixelBuffer() {
            var image: CGImage?
            if VTCreateCGImageFromCVPixelBuffer(buffer, options: nil, imageOut: &image) == noErr {
                backdropImage = image
                backdropLayer.contents = image
            }
        }
        videoSource = geometry.src
        decodedVideoSize = geometry.decoded
        videoSize = ViewportGeometry.logicalVideoSize(decoded: geometry.decoded, src: geometry.src)
        updateVideoContentRect(force: true)
        let renderer = SampleBufferTarget(layer: displayLayer)
        if renderer.status == .failed || renderer.requiresFlushToResumeDecoding { renderer.flush() }
        renderer.enqueue(sample)
        if ViewportGeometry.isFull(geometry.src), renderer.status != .failed { clearBackdrop() }
        refreshMagnifier(pixelBuffer: CMSampleBufferGetImageBuffer(sample))
        CATransaction.commit()
    }

    private func clearBackdrop() {
        backdropImage = nil
        backdropLayer.contents = nil
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        frameRateController.updateVisibility()
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
        reportViewport()
        if window == nil {
            magnifierLayer.show(false)
            pencilGestures?.cancel()
            pencilHoverLayer.hide()
        }
    }

    private func updateVideoContentRect(force: Bool = false) {
        let oldSize = zoomController.viewBounds.size
        let sizeChanged = oldSize != bounds.size
        let nextContentRect = SessionContentRect.fit(video: videoSize, in: bounds.size)
        guard force || zoomController.viewBounds != bounds || videoContentRect != nextContentRect else { return }
        if sizeChanged, let savedViewportForKeyboard {
            self.savedViewportForKeyboard = SessionZoomMath.relayout(
                viewport: savedViewportForKeyboard, oldSize: oldSize, newSize: bounds.size)
        }
        // Resizing changes geometry only. Keep both the decoder and the
        // displayed sample buffer alive while the split divider is dragged.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        contentLayer.bounds = CGRect(origin: .zero, size: bounds.size)
        contentLayer.position = .zero
        videoContentRect = nextContentRect
        backdropLayer.frame = videoContentRect
        displayLayer.frame = ViewportGeometry.cropFrame(contentRect: videoContentRect, src: videoSource)
        displayLayer.videoGravity = ViewportGeometry.isFull(videoSource) ? .resizeAspect : .resize
        zoomController.updateLayout(contentRect: videoContentRect, viewBounds: bounds)
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        if sizeChanged { magnifierLayer.show(false) }
        if keyboardInset > 0 {
            zoomController.setKeyboardTarget(cursor: cursorController.position, inset: keyboardInset,
                                             animated: false, emit: false)
        }
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        refreshMagnifier()
        CATransaction.commit()
        zoomController.emitViewport(force: true)
        reportViewport()
    }

    func consume(_ event: SessionEvent) {
        if event.kind == RS_EV_STREAM_CONFIG {
            markSessionAttached()
            reportViewport()
        }
        cursorSink.consume(event)
        syncCursorLayer()
        refreshMagnifier()
    }

    private func markSessionAttached() {
        guard let sessionId else { return }
        if !inputAttached {
            inputAttached = true
            if UIApplication.shared.applicationState == .active { inputSender.setSession(sessionId) }
            cursorController.setAttached(true)
            lastPointerLockState = nil
        }
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
    }

    func reportPointerLock(mouse: Bool, locked: Bool, reason: PointerLockReason) {
        let state = PointerLockState(mouse: mouse, locked: locked, reason: reason)
        guard state != lastPointerLockState else { return }
        lastPointerLockState = state
        onPointerLock?(["mouse": mouse, "locked": locked, "reason": reason.rawValue])
    }

    func pointerRouteDidChange() { pointerGestures?.routeDidChange() }

    func pointerRelativeMove(raw: CGPoint, now: TimeInterval) {
        guard sessionInputReady, PointerLockCoordinator.shared.activeSurface === self,
              PointerLockCoordinator.shared.route == .relative else { return }
        let accelerated = pointerMotion.viewDelta(raw: raw, now: now)
        let delta = prefs.pointerDelta(CGVector(dx: accelerated.x, dy: accelerated.y))
        if lastPointerMoveLog.map({ now - $0 >= Self.pointerMoveLogInterval }) ?? true {
            lastPointerMoveLog = now
            let length = hypot(raw.x, raw.y)
            let gain = length > 0 ? hypot(accelerated.x, accelerated.y) / length : 1
            RsLog.marker("POINTER", ["move": [Double(raw.x), Double(raw.y)], "gain": Double(gain)])
        }
        if pointerEscape.feed(atBottomEdge: cursorController.position.y >= 1 - 1e-4,
                              viewDelta: delta, now: now) {
            PointerLockCoordinator.shared.escape()
            return
        }
        let position = cursorController.move(viewDelta: CGPoint(x: delta.dx, y: delta.dy), gain: 1, now: now)
        inputSender.pointer(position)
        updateMagnifierCursor(position)
        followCursorIfNeeded()
    }

    func pointerButton(_ button: Int32, down: Bool, now: TimeInterval) {
        let count: UInt8
        if down {
            guard sessionInputReady, pointerButtons[button] == nil else {
                RsLog.marker("POINTER", ["drop": Int(button), "ready": sessionInputReady,
                                         "held": pointerButtons[button] != nil])
                return
            }
            count = button == PointerButtons.left
                ? UInt8(pointerClicks.register(position: cursorController.position, time: now)) : 1
            pointerButtons[button] = count
        } else {
            guard let pressedCount = pointerButtons.removeValue(forKey: button) else { return }
            count = pressedCount
        }
        inputSender.button(button, down: down, at: cursorController.position, clicks: count)
    }

    /// No GameController scroll lately; PointerGestureController may carry UIKit's.
    var lockedScrollQuiet: Bool {
        lastLockedScroll.map { CACurrentMediaTime() - $0 >= Self.lockedScrollQuietAfter } ?? true
    }

    func pointerLockedScroll(x: CGFloat, y: CGFloat, now: TimeInterval) {
        guard sessionInputReady, PointerLockCoordinator.shared.activeSurface === self,
              PointerLockCoordinator.shared.route == .relative else { return }
        lastLockedScroll = now
        pointerGestures?.yieldScroll()
        guard let output = pointerScroll.feed(x: x, y: y, now: now) else { return }
        stopPointerMomentum()
        sendPointerScroll(output)
        schedulePointerScrollEnd()
    }

    private func sendPointerScroll(_ output: PointerScrollSynth.Output) {
        let delta = prefs.scrollDelta(CGVector(dx: output.dx, dy: output.dy))
        inputSender.scroll(dx: delta.dx, dy: delta.dy, phase: output.phase)
    }

    private func schedulePointerScrollEnd() {
        guard pointerScrollWork == nil else { return }
        let generation = pointerScrollGeneration
        let work = DispatchWorkItem { [weak self] in
            guard let self, generation == self.pointerScrollGeneration else { return }
            self.pointerScrollWork = nil
            if let output = self.pointerScroll.idle(now: CACurrentMediaTime()) {
                self.sendPointerScroll(output)
                self.startPointerMomentum(velocity: output.velocity)
            } else {
                self.schedulePointerScrollEnd()
            }
        }
        pointerScrollWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + PointerScrollSynth.idleEnd, execute: work)
    }

    /// GameController scrolls stop dead when the fingers lift; carry a fast
    /// one on the way UIKit's trackpad scrolls do.
    private func startPointerMomentum(velocity: CGVector) {
        let summary = pointerScroll.summary
        RsLog.marker("POINTER", ["scrollEnd": summary.samples, "y": Double(summary.total.dy),
                                 "peak": Double(summary.peak),
                                 "ms": Int(summary.duration * 1000), "vy": Double(velocity.dy)])
        // TrackpadScrollMomentum doubles what it is given (the pixel multiplier).
        pointerScrollMomentum.start(velocity: CGPoint(x: velocity.dx / 2, y: velocity.dy / 2))
        guard pointerScrollMomentum.isActive else { return }
        let link = CADisplayLink(target: pointerMomentumTarget, selector: #selector(PointerMomentumTarget.tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        pointerMomentumLink = link
        link.add(to: .main, forMode: .common)
    }

    fileprivate func stepPointerMomentum(_ link: CADisplayLink) {
        guard sessionInputReady, PointerLockCoordinator.shared.activeSurface === self,
              PointerLockCoordinator.shared.route == .relative else { stopPointerMomentum(); return }
        let dt = lastPointerMomentumTime.map { link.timestamp - $0 } ?? 1 / 60
        lastPointerMomentumTime = link.timestamp
        let step = pointerScrollMomentum.step(dt: dt)
        if step.phase != 0 {
            let delta = prefs.scrollDelta(CGVector(dx: step.dx, dy: step.dy))
            inputSender.scroll(dx: delta.dx, dy: delta.dy, momentum: step.phase)
        }
        if step.finished {
            pointerMomentumLink?.invalidate()
            pointerMomentumLink = nil
            lastPointerMomentumTime = nil
        }
    }

    private func stopPointerMomentum() {
        if pointerScrollMomentum.stop() { inputSender.scroll(dx: 0, dy: 0, momentum: 3) }
        pointerMomentumLink?.invalidate()
        pointerMomentumLink = nil
        lastPointerMomentumTime = nil
    }

    func cancelPointerInput() {
        pointerMotion.reset()
        pointerEscape.reset()
        pointerScrollGeneration &+= 1
        pointerScrollWork?.cancel()
        pointerScrollWork = nil
        if let output = pointerScroll.reset() { sendPointerScroll(output) }
        stopPointerMomentum()
        pointerGestures?.cancel()
        for (button, count) in pointerButtons.sorted(by: { $0.key < $1.key }) {
            inputSender.button(button, down: false, at: cursorController.position, clicks: count)
        }
        pointerButtons.removeAll()
        pointerClicks = TrackpadClickCounter()
        inputSender.flushPointer()
    }

    func reportGesture(_ kind: String) {
        RsLog.marker("SESSVIEW-GESTURE", ["kind": kind])
        onGesture?(["kind": kind])
    }

    func handlePinch(scale: CGFloat, location: CGPoint, state: UIGestureRecognizer.State, report: Bool) {
        zoomController.pinch(scale: scale, location: location, state: state)
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        refreshMagnifier()
        if report { reportGesture("zoom") }
    }

    func finishViewportGesture() {
        zoomController.emitViewport(force: true)
    }

    func followCursorIfNeeded() {
        let relativePointer = PointerLockCoordinator.shared.activeSurface === self
            && PointerLockCoordinator.shared.route == .relative
        guard mode == "trackpad" || keyboardInset > 0 || relativePointer else { return }
        zoomController.followCursor(cursorController.position)
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        refreshMagnifier()
    }

    func updateMagnifierCursor(_ position: CGPoint) {
        syncCursorLayer()
        guard magnifierLayer.isHidden == false else { return }
        refreshMagnifier()
    }

    func setMagnifierVisible(_ visible: Bool, at point: CGPoint? = nil) {
        if let point { updateMagnifierCursorAtViewPoint(point) }
        refreshMagnifier()
        magnifierLayer.show(visible)
    }

    @objc func showMagnifier(_ on: Bool) {
        refreshMagnifier()
        magnifierLayer.show(on)
        RsLog.marker("SESSVIEW", ["command": "showMagnifier", "on": on, "impl": true])
    }

    private func updateMagnifierCursorAtViewPoint(_ point: CGPoint) {
        let normalized = TouchTapMapper.normalized(viewPoint: point, viewport: cursorController.viewport)
        let current = cursorController.position
        let target = cursorController.viewport.viewPoint(fromNormalized: normalized)
        let currentPoint = cursorController.viewport.viewPoint(fromNormalized: current)
        _ = cursorController.move(viewDelta: CGPoint(x: target.x - currentPoint.x, y: target.y - currentPoint.y),
                                  gain: 1, now: CACurrentMediaTime())
        syncCursorLayer()
    }

    private func syncCursorLayer() {
        cursorController.updateTransform(zoomController.currentTransform())
    }

    private func refreshMagnifier(pixelBuffer: CVPixelBuffer? = nil) {
        let point = zoomController.screenPoint(forNormalized: cursorController.position)
        let ppp = decodedVideoSize.width > 0
            ? videoContentRect.width * videoSource.width / decodedVideoSize.width * zoomController.viewport.zoom
            : 1
        magnifierLayer.update(cursor: cursorController.position, viewport: zoomController.viewport,
                              videoSize: decodedVideoSize, src: videoSource, backdrop: backdropImage,
                              cursorViewPoint: point, frame: bounds,
                              pointsPerVideoPixel: ppp, reduceMotion: reduceMotion, pixelBuffer: pixelBuffer)
    }

    private func modeDidChange() {
        cancelPointerInput()
        pencilGestures?.cancel()
        trackpadGestures?.uninstall()
        touchGestures?.uninstall()
        if mode == "trackpad" { trackpadGestures?.install() }
        else { touchGestures?.install() }
    }

    @objc private func willResignActive() {
        pointerApplicationActive = false
        cancelPointerInput()
        pointerGestures?.setSuspended(true)
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
        trackpadGestures?.setSuspended(true)
        touchGestures?.setSuspended(true)
        pencilGestures?.setSuspended(true)
        spaceSwipe?.setSuspended(true)
        inputSender.setSession(nil)
    }

    @objc private func sceneWillDeactivate(_ notification: Notification) {
        guard let scene = notification.object as? UIScene, scene === window?.windowScene,
              pointerApplicationActive else { return }
        willResignActive()
    }

    @objc private func sceneDidActivate(_ notification: Notification) {
        guard let scene = notification.object as? UIScene, scene === window?.windowScene else { return }
        activate()
    }

    @objc private func didBecomeActive() {
        // The app is active when any of its windows is; this one may not be.
        if let scene = window?.windowScene, scene.activationState != .foregroundActive { return }
        activate()
    }

    private func activate() {
        guard !pointerApplicationActive else { return }
        pointerApplicationActive = true
        inputSender.setSession(inputAttached ? sessionId : nil)
        trackpadGestures?.setSuspended(false)
        touchGestures?.setSuspended(false)
        pencilGestures?.setSuspended(false)
        pointerGestures?.setSuspended(false)
        spaceSwipe?.setSuspended(false)
        PointerLockCoordinator.shared.surfaceDidChange(self)
        SessionPipCoordinator.shared.surfaceDidChange(self)
    }

    private func keyboardInsetDidChange() {
        let inset = max(0, keyboardInset)
        zoomController.keyboardInset = inset
        if inset > 0 {
            if savedViewportForKeyboard == nil { savedViewportForKeyboard = zoomController.viewport }
            zoomController.reduceMotion = reduceMotion
            zoomController.setKeyboardTarget(cursor: cursorController.position, inset: inset)
        } else if let savedViewportForKeyboard {
            self.savedViewportForKeyboard = nil
            zoomController.restore(savedViewportForKeyboard)
        }
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        refreshMagnifier()
        reportViewport()
    }

    private func reduceMotionDidChange() {
        zoomController.reduceMotion = reduceMotion
        refreshMagnifier()
    }

    @objc func zoomReset() {
        zoomController.reduceMotion = reduceMotion
        zoomController.reset()
        cursorController.update(viewport: zoomController.viewport, videoSize: videoSize)
        syncCursorLayer()
        refreshMagnifier()
        RsLog.marker("SESSVIEW", ["command": "zoomReset", "impl": true])
    }

    @objc func centerCursor() {
        cursorController.center(now: CACurrentMediaTime())
        inputSender.pointer(cursorController.position)
        inputSender.flushPointer()
        followCursorIfNeeded()
        refreshMagnifier()
        RsLog.marker("SESSVIEW", ["command": "centerCursor", "impl": true])
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        cancelPointerInput()
        pointerGestures?.uninstall()
        trackpadGestures?.uninstall()
        touchGestures?.uninstall()
        pencilGestures?.uninstall()
        inputSender.setSession(nil)
        attachment.detach()
        pipeline?.stop()
    }
}

/// The display link keeps its target; this one does not keep the surface.
private final class PointerMomentumTarget: NSObject {
    weak var surface: RemoteSessionSurface?
    @objc func tick(_ link: CADisplayLink) { surface?.stepPointerMomentum(link) }
}
