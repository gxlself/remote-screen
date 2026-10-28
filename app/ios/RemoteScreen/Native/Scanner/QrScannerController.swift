import AVFoundation
import Foundation
import UIKit

/// UIKit and callbacks stay on main; all capture work and scanner logic stay
/// on sessionQueue. Revisions prevent queued results from escaping after stop.
@objc(QrScannerController)
final class QrScannerController: NSObject {
    private let containerView: UIView
    private let session = AVCaptureSession()
    private let sessionQueue = DispatchQueue(label: "rs.scanner.session", qos: .userInitiated)
    private let previewLayer: AVCaptureVideoPreviewLayer
    private lazy var metadataDelegate = QrScannerMetadataDelegate(scanner: self)
    private var observers: [NSObjectProtocol] = []

    // Main queue only.
    private var active = false
    private var attached = false
    private var foreground: Bool
    private var revision: UInt64 = 0

    // sessionQueue only.
    private var captureRevision: UInt64 = 0
    private var shouldRun = false
    private var configured = false
    private var camera: AVCaptureDevice?
    private var torchRequested = false
    private var requestingPermission = false
    private var interrupted = false
    private var lastState: String?
    private var throttle = QrScanThrottle()

    @objc var onCode: ((String) -> Void)?
    @objc var onScannerState: ((String, String, Bool) -> Void)?

    @objc(initWithView:)
    init(view: UIView) {
        containerView = view
        foreground = UIApplication.shared.applicationState != .background
        previewLayer = AVCaptureVideoPreviewLayer(session: session)
        super.init()
        previewLayer.videoGravity = .resizeAspectFill
        containerView.layer.addSublayer(previewLayer)

        observe(UIApplication.didEnterBackgroundNotification) { scanner, _ in
            // Keep `active` so returning to the foreground can resume scanning.
            scanner.foreground = false
            scanner.refreshActivity()
        }
        observe(UIApplication.didBecomeActiveNotification) { scanner, _ in
            scanner.foreground = true
            scanner.refreshActivity()
        }
        observe(AVCaptureSession.wasInterruptedNotification, object: session) { scanner, _ in
            scanner.sessionQueue.async { [weak scanner] in
                guard let scanner else { return }
                scanner.interrupted = true
                scanner.torchRequested = false
                scanner.applyTorch()
                if scanner.shouldRun { scanner.emitState("starting") }
            }
        }
        observe(AVCaptureSession.interruptionEndedNotification, object: session) { scanner, _ in
            scanner.sessionQueue.async { [weak scanner] in
                guard let scanner else { return }
                scanner.interrupted = false
                scanner.reconcile()
            }
        }
        observe(AVCaptureSession.runtimeErrorNotification, object: session) { scanner, notification in
            let error = notification.userInfo?[AVCaptureSessionErrorKey] as? AVError
            scanner.sessionQueue.async { [weak scanner] in
                guard let scanner, scanner.shouldRun else { return }
                if error?.code == .mediaServicesWereReset {
                    scanner.reconcile()
                } else {
                    scanner.emitState("error", message: error?.localizedDescription ?? "Camera session failed")
                }
            }
        }
    }

    deinit {
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        // Capture the session itself: a weak-self closure would skip the stop
        // once the Fabric component has released its controller.
        let captureSession = session
        sessionQueue.async {
            if let camera = captureSession.inputs
                .compactMap({ ($0 as? AVCaptureDeviceInput)?.device })
                .first(where: { $0.hasTorch }) {
                Self.setTorch(false, on: camera)
            }
            if captureSession.isRunning { captureSession.stopRunning() }
        }
    }

    @objc(setActive:)
    func setActive(_ value: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        active = value
        refreshActivity()
    }

    @objc(setTorch:)
    func setTorch(_ value: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        sessionQueue.async { [weak self] in
            guard let self else { return }
            guard self.shouldRun else {
                self.torchRequested = false
                Self.setTorch(false, on: self.camera)
                return
            }
            self.torchRequested = value
            self.applyTorch()
        }
    }

    @objc(setAttached:)
    func setAttached(_ value: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        attached = value
        refreshActivity()
    }

    @objc(layout)
    func layout() {
        dispatchPrecondition(condition: .onQueue(.main))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        previewLayer.frame = containerView.bounds
        if let connection = previewLayer.connection {
            let angle: CGFloat
            switch containerView.window?.windowScene?.interfaceOrientation {
            case .landscapeLeft: angle = 0
            case .landscapeRight: angle = 180
            case .portraitUpsideDown: angle = 270
            default: angle = 90
            }
            if #available(iOS 17, *) {
                if connection.isVideoRotationAngleSupported(angle) {
                    connection.videoRotationAngle = angle
                }
            } else if connection.isVideoOrientationSupported {
                // Rotation angle 0 is the sensor's native landscapeRight.
                switch angle {
                case 0: connection.videoOrientation = .landscapeRight
                case 180: connection.videoOrientation = .landscapeLeft
                case 270: connection.videoOrientation = .portraitUpsideDown
                default: connection.videoOrientation = .portrait
                }
            }
        }
        CATransaction.commit()
    }

    @objc(stop)
    func stop() {
        setActive(false)
    }

    private func observe(
        _ name: Notification.Name,
        object: Any? = nil,
        handler: @escaping (QrScannerController, Notification) -> Void
    ) {
        observers.append(NotificationCenter.default.addObserver(
            forName: name,
            object: object,
            queue: .main
        ) { [weak self] notification in
            guard let self else { return }
            handler(self, notification)
        })
    }

    private func refreshActivity() {
        revision &+= 1
        let nextRevision = revision
        let nextShouldRun = active && attached && foreground
        sessionQueue.async { [weak self] in
            guard let self else { return }
            self.captureRevision = nextRevision
            self.shouldRun = nextShouldRun
            self.lastState = nil
            if nextShouldRun {
                self.reconcile()
            } else {
                self.torchRequested = false
                self.applyTorch()
                if self.session.isRunning { self.session.stopRunning() }
            }
        }
    }

    private func reconcile() {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        guard shouldRun else { return }
        let camera = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
        let status: ScannerPermissionStatus
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: status = .authorized
        case .notDetermined: status = .notDetermined
        case .denied: status = .denied
        case .restricted: status = .restricted
        @unknown default: status = .restricted
        }
        switch ScannerPermission.decide(status: status, hasDevice: camera != nil) {
        case .unavailable:
            emitState("unavailable")
        case .denied:
            emitState("denied")
        case .restricted:
            emitState("restricted")
        case .request:
            emitState("starting")
            guard !requestingPermission else { return }
            requestingPermission = true
            AVCaptureDevice.requestAccess(for: .video) { [weak self] _ in
                self?.sessionQueue.async { [weak self] in
                    guard let self else { return }
                    self.requestingPermission = false
                    self.reconcile()
                }
            }
        case .start:
            emitState("starting")
            do {
                if !configured {
                    guard let camera else { return }
                    try configure(camera)
                }
                guard !interrupted, !session.isInterrupted else { return }
                if !session.isRunning { session.startRunning() }
                if session.isRunning {
                    emitState("running", hasTorch: self.camera?.hasTorch == true)
                    applyTorch()
                } else if !session.isInterrupted {
                    emitState("error", message: "Camera session did not start")
                }
            } catch {
                emitState("error", message: error.localizedDescription)
            }
        }
    }

    private func configure(_ camera: AVCaptureDevice) throws {
        let input = try AVCaptureDeviceInput(device: camera)
        let output = AVCaptureMetadataOutput()
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        guard session.canAddInput(input) else { throw ScannerError.inputUnavailable }
        session.addInput(input)
        guard session.canAddOutput(output) else {
            session.removeInput(input)
            throw ScannerError.outputUnavailable
        }
        session.addOutput(output)
        guard output.availableMetadataObjectTypes.contains(.qr) else {
            session.removeOutput(output)
            session.removeInput(input)
            throw ScannerError.qrUnavailable
        }
        output.setMetadataObjectsDelegate(metadataDelegate, queue: sessionQueue)
        output.metadataObjectTypes = [.qr]
        // Leave rectOfInterest at its full-sensor default. The four corners
        // guide positioning; scanning is not clipped to the decorative frame.
        self.camera = camera
        configured = true
    }

    private func applyTorch() {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        guard shouldRun, session.isRunning, !interrupted, !session.isInterrupted, torchRequested else {
            Self.setTorch(false, on: camera)
            return
        }
        guard let camera, camera.hasTorch else {
            torchRequested = false
            return
        }
        guard Self.setTorch(true, on: camera) else {
            torchRequested = false
            Self.setTorch(false, on: camera)
            return
        }
    }

    @discardableResult
    private static func setTorch(_ enabled: Bool, on camera: AVCaptureDevice?) -> Bool {
        guard let camera, camera.hasTorch else { return !enabled }
        let mode: AVCaptureDevice.TorchMode = enabled ? .on : .off
        guard camera.isTorchModeSupported(mode) else { return false }
        guard camera.torchMode != mode else { return true }
        do {
            try camera.lockForConfiguration()
            defer { camera.unlockForConfiguration() }
            camera.torchMode = mode
            return true
        } catch {
            return false
        }
    }

    private func emitState(
        _ state: String,
        message: String = "",
        hasTorch: Bool = false
    ) {
        if state != "running" {
            torchRequested = false
            Self.setTorch(false, on: camera)
        }
        guard lastState != state else { return }
        lastState = state
        let eventRevision = captureRevision
        DispatchQueue.main.async { [weak self] in
            guard let self, self.revision == eventRevision,
                  self.active, self.attached, self.foreground else { return }
            self.layout()
            RsLog.marker("SCANNER", ["state": state])
            self.onScannerState?(state, message, hasTorch)
        }
    }

    fileprivate func handleMetadata(_ metadataObjects: [AVMetadataObject]) {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        guard shouldRun, !interrupted else { return }
        for case let code as AVMetadataMachineReadableCodeObject in metadataObjects {
            guard code.type == .qr, let text = code.stringValue, !text.isEmpty else { continue }
            let nowMs = UInt64(Date().timeIntervalSince1970 * 1_000)
            guard throttle.accept(text, nowMs: nowMs) else { continue }
            let eventRevision = captureRevision
            DispatchQueue.main.async { [weak self] in
                guard let self, self.revision == eventRevision,
                      self.active, self.attached, self.foreground else { return }
                RsLog.marker("SCANNER", ["code": String(text.prefix(16)) + "…", "len": text.count])
                self.onCode?(text)
            }
        }
    }
}

// Keep AVFoundation protocols out of the shared RemoteScreen-Swift.h. Other
// native ObjC++ files import it with C++ modules disabled.
private final class QrScannerMetadataDelegate: NSObject, AVCaptureMetadataOutputObjectsDelegate {
    private weak var scanner: QrScannerController?

    init(scanner: QrScannerController) {
        self.scanner = scanner
        super.init()
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        scanner?.handleMetadata(metadataObjects)
    }
}

private enum ScannerError: LocalizedError {
    case inputUnavailable
    case outputUnavailable
    case qrUnavailable

    var errorDescription: String? {
        switch self {
        case .inputUnavailable: return "Cannot add camera input"
        case .outputUnavailable: return "Cannot add metadata output"
        case .qrUnavailable: return "QR recognition is unavailable"
        }
    }
}
