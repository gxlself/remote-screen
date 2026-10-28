import Foundation
import AVFoundation
import AVKit
import CoreMedia

/// Bridges one AVSampleBufferDisplayLayer to the system Picture in Picture
/// controller. All methods and callbacks are expected on the main thread.
final class SessionPipController: NSObject {
    private let isPaused: () -> Bool
    private let sink: (PipEvent) -> Void
    private var pictureInPictureController: AVPictureInPictureController?
    private var delegateProxy: PipDelegateProxy?
    private var possibleObservation: NSKeyValueObservation?
    private var restoreCompletion: ((Bool) -> Void)?
    private var restoreTimeout: DispatchWorkItem?

    init(
        layer: AVSampleBufferDisplayLayer,
        isPaused: @escaping () -> Bool,
        sink: @escaping (PipEvent) -> Void
    ) {
        self.isPaused = isPaused
        self.sink = sink
        super.init()

        let delegateProxy = PipDelegateProxy(owner: self)
        self.delegateProxy = delegateProxy
        let source = AVPictureInPictureController.ContentSource(
            sampleBufferDisplayLayer: layer,
            playbackDelegate: delegateProxy
        )
        let controller = AVPictureInPictureController(contentSource: source)
        controller.requiresLinearPlayback = true
        controller.canStartPictureInPictureAutomaticallyFromInline = false
        controller.delegate = delegateProxy
        pictureInPictureController = controller
        possibleObservation = controller.observe(
            \.isPictureInPicturePossible,
            options: [.initial, .new]
        ) { [weak self] controller, _ in
            guard let self else { return }
            let possible = controller.isPictureInPicturePossible
            self.mark("possible", reason: String(possible))
            self.sink(.possibleChanged(possible))
        }
    }

    func setAutoStart(_ on: Bool) {
        pictureInPictureController?.canStartPictureInPictureAutomaticallyFromInline = on
    }

    func start() {
        pictureInPictureController?.startPictureInPicture()
    }

    func stop() {
        pictureInPictureController?.stopPictureInPicture()
    }

    func invalidatePlaybackState() {
        pictureInPictureController?.invalidatePlaybackState()
    }

    func completeRestore(_ ok: Bool) {
        guard let completion = restoreCompletion else { return }
        restoreCompletion = nil
        restoreTimeout?.cancel()
        restoreTimeout = nil
        completion(ok)
    }

    func tearDown() {
        pictureInPictureController?.stopPictureInPicture()
        possibleObservation?.invalidate()
        possibleObservation = nil
        restoreTimeout?.cancel()
        restoreTimeout = nil
        if restoreCompletion != nil {
            completeRestore(false)
        }
        pictureInPictureController?.delegate = nil
        pictureInPictureController = nil
        delegateProxy = nil
    }

    deinit {
        possibleObservation?.invalidate()
        restoreTimeout?.cancel()
        pictureInPictureController?.delegate = nil
        delegateProxy?.owner = nil
    }

    private func mark(_ event: String, reason: String = "") {
        RsLog.marker("PIP", ["phase": "controller", "event": event, "reason": reason])
    }
    fileprivate func delegateWillStart() {
        mark("willStart")
        sink(.willStart)
    }

    fileprivate func delegateDidStart() {
        mark("started")
        sink(.didStart)
    }

    fileprivate func delegateFailedToStart(error: Error) {
        let code = (error as NSError).code
        mark("failed", reason: "errorCode:\(code)")
        sink(.failedToStart)
    }

    fileprivate func delegateWillStop() {
        mark("willStop")
        sink(.willStop)
    }

    fileprivate func delegateDidStop() {
        mark("stopped")
        sink(.didStop)
    }

    fileprivate func delegateRestore(completionHandler: @escaping (Bool) -> Void) {
        completeRestore(false)
        restoreCompletion = completionHandler
        let timeout = DispatchWorkItem { [weak self] in
            self?.completeRestore(false)
        }
        restoreTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: timeout)
        mark("restore")
        sink(.restoreRequested)
    }

    fileprivate func delegateSetPlaying(_ playing: Bool) {
        mark("setPlaying", reason: playing ? "playing" : "paused")
        sink(.setPlaying(playing))
    }

    fileprivate func delegateTimeRangeForPlayback() -> CMTimeRange {
        CMTimeRange(start: .negativeInfinity, duration: .positiveInfinity)
    }

    fileprivate func delegateIsPlaybackPaused() -> Bool {
        isPaused()
    }

    fileprivate func delegateDidTransition(to newRenderSize: CMVideoDimensions) {
        mark("renderSize", reason: "\(newRenderSize.width)x\(newRenderSize.height)")
    }

    fileprivate func delegateSkip(completion: @escaping () -> Void) {
        completion()
    }
}

private final class PipDelegateProxy: NSObject,
    AVPictureInPictureControllerDelegate,
    AVPictureInPictureSampleBufferPlaybackDelegate {
    weak var owner: SessionPipController?

    init(owner: SessionPipController) {
        self.owner = owner
        super.init()
    }

    func pictureInPictureControllerWillStartPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        owner?.delegateWillStart()
    }

    func pictureInPictureControllerDidStartPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        owner?.delegateDidStart()
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        failedToStartPictureInPictureWithError error: Error
    ) {
        owner?.delegateFailedToStart(error: error)
    }

    func pictureInPictureControllerWillStopPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        owner?.delegateWillStop()
    }

    func pictureInPictureControllerDidStopPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        owner?.delegateDidStop()
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping (Bool) -> Void
    ) {
        owner?.delegateRestore(completionHandler: completionHandler)
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        setPlaying playing: Bool
    ) {
        owner?.delegateSetPlaying(playing)
    }

    func pictureInPictureControllerTimeRangeForPlayback(
        _ pictureInPictureController: AVPictureInPictureController
    ) -> CMTimeRange {
        owner?.delegateTimeRangeForPlayback()
            ?? CMTimeRange(start: .negativeInfinity, duration: .positiveInfinity)
    }

    func pictureInPictureControllerIsPlaybackPaused(
        _ pictureInPictureController: AVPictureInPictureController
    ) -> Bool {
        owner?.delegateIsPlaybackPaused() ?? false
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        didTransitionToRenderSize newRenderSize: CMVideoDimensions
    ) {
        owner?.delegateDidTransition(to: newRenderSize)
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        skipByInterval skipInterval: CMTime,
        completion: @escaping () -> Void
    ) {
        owner?.delegateSkip(completion: completion) ?? completion()
    }
}
