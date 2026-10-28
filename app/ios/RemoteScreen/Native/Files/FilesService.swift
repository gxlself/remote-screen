import Foundation
import React
import UIKit
import UniformTypeIdentifiers

@objc(FilesService)
final class FilesService: NSObject {
    @objc(shared)
    static let shared = FilesService()

    private enum PickerOperation {
        case pick(([NSDictionary], NSError?) -> Void)
        case save((Bool, NSError?) -> Void)
    }

    private let fileManager = FileManager.default
    private let copyQueue = DispatchQueue(
        label: "ai.gxlself.remotescreen.files.copy",
        qos: .userInitiated
    )
    private var activePicker: UIDocumentPickerViewController?
    private var pickerOperation: PickerOperation?
    private var backgroundGate = BackgroundTaskGate()
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    private override init() {
        super.init()
    }

    @objc(getConstants)
    func getConstants() -> NSDictionary {
        let directories = directoryURLs()
        do {
            try ensureDirectories(directories)
        } catch {
            RsLog.marker("FILES", ["directories": "error"])
        }
        return [
            "filesDir": directories.files.path,
            "inboxDir": directories.inbox.path,
        ] as NSDictionary
    }

    @objc(pickFilesWithCompletion:)
    func pickFiles(completion: @escaping (NSArray, NSError?) -> Void) {
        performOnMain { [weak self] in
            guard let self else { return }
            guard self.activePicker == nil else {
                completion([], self.makeError(code: "busy", message: "文件选择器正在显示"))
                return
            }

            let directories = self.directoryURLs()
            do {
                try self.ensureDirectories(directories)
            } catch {
                completion([], self.makeError(code: "directories", message: "无法准备文件目录"))
                return
            }

            let picker = UIDocumentPickerViewController(
                forOpeningContentTypes: [.item],
                asCopy: true
            )
            picker.allowsMultipleSelection = true
            self.present(
                picker,
                operation: .pick { files, error in
                    completion(files as NSArray, error)
                },
                onFailure: { error in
                    completion([], error)
                }
            )
        }
    }

    @objc(saveToFilesWithUri:name:completion:)
    func saveToFiles(
        uri: String,
        name: String,
        completion: @escaping (Bool, NSError?) -> Void
    ) {
        performOnMain { [weak self] in
            guard let self else { return }
            guard self.activePicker == nil else {
                completion(false, self.makeError(code: "busy", message: "文件选择器正在显示"))
                return
            }
            var isDirectory: ObjCBool = false
            guard let source = URL(string: uri),
                  source.isFileURL,
                  self.fileManager.fileExists(atPath: source.path, isDirectory: &isDirectory),
                  !isDirectory.boolValue else {
                completion(false, self.makeError(code: "not_found", message: "文件不存在"))
                return
            }

            // UIDocumentPicker uses the source file name in its export UI. The
            // caller still passes the display name so the native boundary has
            // one stable signature; the URI's basename is authoritative here.
            _ = name
            let picker = UIDocumentPickerViewController(
                forExporting: [source],
                asCopy: true
            )
            self.present(
                picker,
                operation: .save { saved, error in
                    completion(saved, error)
                },
                onFailure: { error in
                    completion(false, error)
                }
            )
        }
    }

    @objc(importIncomingWithUrl:completion:)
    func importIncoming(
        url: String,
        completion: @escaping (NSDictionary?, NSError?) -> Void
    ) {
        guard let source = URL(string: url), IncomingURL.isFile(source) else {
            performOnMain {
                completion(nil, nil)
            }
            return
        }

        copyQueue.async { [weak self] in
            guard let self else { return }
            do {
                let result = try self.copyIncoming(source: source)
                DispatchQueue.main.async {
                    completion(result, nil)
                }
            } catch {
                DispatchQueue.main.async {
                    completion(
                        nil,
                        self.makeError(code: "import_failed", message: error.localizedDescription)
                    )
                }
            }
        }
    }

    @objc(setTransfersActive:)
    func setTransfersActive(_ active: Bool) {
        performOnMain { [weak self] in
            self?.updateBackgroundTask(active: active)
        }
    }

    // MARK: File operations

    private func directoryURLs() -> (files: URL, inbox: URL) {
        let documents = fileManager.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let files = documents.appendingPathComponent("Received", isDirectory: true)
        let inbox = fileManager.temporaryDirectory.appendingPathComponent(
            "RsInbox",
            isDirectory: true
        )
        return (files, inbox)
    }

    private func ensureDirectories(
        _ directories: (files: URL, inbox: URL)
    ) throws {
        try fileManager.createDirectory(
            at: directories.files,
            withIntermediateDirectories: true,
            attributes: nil
        )
        try fileManager.createDirectory(
            at: directories.inbox,
            withIntermediateDirectories: true,
            attributes: nil
        )
    }

    private func movePickedFile(from source: URL) throws -> NSDictionary {
        let directories = directoryURLs()
        try ensureDirectories(directories)
        let name = source.lastPathComponent.isEmpty ? "file" : source.lastPathComponent
        let destination = try uniqueDestination(
            directory: directories.inbox,
            requestedName: name
        )
        let scoped = source.startAccessingSecurityScopedResource()
        defer {
            if scoped {
                source.stopAccessingSecurityScopedResource()
            }
        }
        // asCopy gives us a disposable system copy. Move it so selecting a
        // large file does not leave two app-owned temporary copies behind.
        try fileManager.moveItem(at: source, to: destination)
        return fileInfo(for: destination, name: destination.lastPathComponent)
    }

    private func copyIncoming(source: URL) throws -> NSDictionary {
        let directories = directoryURLs()
        try ensureDirectories(directories)
        let scoped = source.startAccessingSecurityScopedResource()
        defer {
            if scoped {
                source.stopAccessingSecurityScopedResource()
            }
        }
        var coordinationError: NSError?
        var copyError: Error?
        var result: NSDictionary?
        let coordinator = NSFileCoordinator(filePresenter: nil)
        coordinator.coordinate(
            readingItemAt: source,
            options: [],
            error: &coordinationError
        ) { coordinatedURL in
            do {
                let name = coordinatedURL.lastPathComponent.isEmpty
                    ? "file"
                    : coordinatedURL.lastPathComponent
                let destination = try self.uniqueDestination(
                    directory: directories.inbox,
                    requestedName: name
                )
                try self.fileManager.copyItem(at: coordinatedURL, to: destination)
                result = self.fileInfo(for: destination, name: destination.lastPathComponent)
            } catch {
                copyError = error
            }
        }

        if let coordinationError {
            throw coordinationError
        }
        if let copyError {
            throw copyError
        }
        guard let result else {
            throw makeError(code: "import_failed", message: "无法读取分享来的文件")
        }

        if isSystemInboxFile(source, documents: directories.files.deletingLastPathComponent()) {
            do {
                try fileManager.removeItem(at: source)
                RsLog.marker("FILES", ["inboxCleanup": "removed"])
            } catch {
                // The usable RsInbox copy must still be returned. Report a
                // cleanup failure without exposing its private source path.
                RsLog.marker("FILES", ["inboxCleanup": "failed"])
            }
        }
        return result
    }

    private func uniqueDestination(directory: URL, requestedName: String) throws -> URL {
        let taken = try Set(
            fileManager
                .contentsOfDirectory(
                    at: directory,
                    includingPropertiesForKeys: nil,
                    options: []
                )
                .map { $0.lastPathComponent }
        )
        let name = UniqueName.next(name: requestedName, taken: taken)
        return directory.appendingPathComponent(name, isDirectory: false)
    }

    private func fileInfo(for url: URL, name: String) -> NSDictionary {
        let values = try? url.resourceValues(forKeys: [.fileSizeKey])
        let size = values?.fileSize ?? -1
        return [
            "uri": url.absoluteString,
            "name": name,
            "size": size,
        ] as NSDictionary
    }

    private func isSystemInboxFile(_ source: URL, documents: URL) -> Bool {
        let inbox = documents.appendingPathComponent("Inbox", isDirectory: true)
            .resolvingSymlinksInPath().standardizedFileURL.path
        let path = source.resolvingSymlinksInPath().standardizedFileURL.path
        return path.hasPrefix(inbox + "/")
    }

    // MARK: Picker presentation

    private func present(
        _ picker: UIDocumentPickerViewController,
        operation: PickerOperation,
        onFailure: (NSError) -> Void
    ) {
        guard let presenter = RCTPresentedViewController(),
              presenter.viewIfLoaded?.window != nil,
              !presenter.isBeingDismissed else {
            onFailure(makeError(code: "no_presenter", message: "无法打开文件选择器"))
            return
        }
        guard !(presenter is UIDocumentPickerViewController) else {
            onFailure(makeError(code: "busy", message: "文件选择器正在显示"))
            return
        }
        picker.delegate = self
        activePicker = picker
        pickerOperation = operation
        presenter.present(picker, animated: true)
        picker.presentationController?.delegate = self
    }

    // MARK: Background execution

    private func updateBackgroundTask(active: Bool) {
        switch backgroundGate.set(active: active) {
        case .begin:
            guard backgroundTask == .invalid else { return }
            let task = UIApplication.shared.beginBackgroundTask(withName: "RsFiles") {
                [weak self] in
                self?.backgroundTaskExpired()
            }
            guard task != .invalid else {
                _ = backgroundGate.expired()
                return
            }
            backgroundTask = task
            RsLog.marker("FILES", ["bg": "begin"])
        case .end:
            endBackgroundTask(reason: "end")
        case .none:
            break
        }
    }

    private func backgroundTaskExpired() {
        performOnMain { [weak self] in
            guard let self,
                  self.backgroundGate.expired() == .end else {
                return
            }
            self.endBackgroundTask(reason: "expired")
        }
    }

    private func endBackgroundTask(reason: String) {
        guard backgroundTask != .invalid else { return }
        let task = backgroundTask
        backgroundTask = .invalid
        UIApplication.shared.endBackgroundTask(task)
        RsLog.marker("FILES", ["bg": reason])
    }

    private func performOnMain(_ action: @escaping () -> Void) {
        if Thread.isMainThread {
            action()
        } else {
            DispatchQueue.main.async(execute: action)
        }
    }

    private func makeError(code: String, message: String) -> NSError {
        NSError(
            domain: code,
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: message]
        )
    }
}

// Keep the UIKit delegate conformance separate so the disk helpers retain
// their copyQueue execution context instead of inferring main-actor isolation.
extension FilesService: UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate {
    func documentPicker(
        _ controller: UIDocumentPickerViewController,
        didPickDocumentsAt urls: [URL]
    ) {
        guard controller === activePicker,
              let operation = pickerOperation else {
            return
        }
        activePicker = nil
        pickerOperation = nil

        switch operation {
        case .save(let completion):
            completion(true, nil)
        case .pick(let completion):
            copyQueue.async { [weak self] in
                guard let self else { return }
                do {
                    let files = try urls.map { try self.movePickedFile(from: $0) }
                    DispatchQueue.main.async {
                        completion(files, nil)
                    }
                } catch {
                    DispatchQueue.main.async {
                        completion(
                            [],
                            self.makeError(code: "copy_failed", message: error.localizedDescription)
                        )
                    }
                }
            }
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard controller === activePicker,
              let operation = pickerOperation else {
            return
        }
        activePicker = nil
        pickerOperation = nil

        switch operation {
        case .pick(let completion):
            completion([], nil)
        case .save(let completion):
            completion(false, nil)
        }
    }

    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        guard let picker = presentationController.presentedViewController
            as? UIDocumentPickerViewController else {
            return
        }
        documentPickerWasCancelled(picker)
    }
}
