import Foundation
import MetricKit

final class CrashNativeCollector {
    static let shared = CrashNativeCollector()

    private let queue = DispatchQueue(label: "ai.gxlself.remotescreen.crash.native")
    private var subscriber: MetricKitSubscriber?

    private init() {}

    func start() {
        guard subscriber == nil else {
            return
        }
        let subscriber = MetricKitSubscriber { [weak self] payloads in
            self?.didReceive(payloads)
        }
        self.subscriber = subscriber
        MXMetricManager.shared.add(subscriber)
    }

    private func didReceive(_ payloads: [MXDiagnosticPayload]) {
        var items: [(Data, UInt64)] = []
        for payload in payloads {
            let incidentAtUs = CrashInbox.incidentMicros(payload.timeStampEnd)
            for diagnostic in payload.crashDiagnostics ?? [] {
                items.append((diagnostic.jsonRepresentation(), incidentAtUs))
            }
        }

        guard !items.isEmpty else {
            return
        }
        queue.async { [weak self] in
            self?.write(items)
        }
    }

    private func write(_ items: [(Data, UInt64)]) {
        let fileManager = FileManager.default
        guard let appSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            RsLog.marker("CRASH", ["source": "metrickit", "error": "dir"])
            return
        }

        let dataDir = LaunchDataDir.url(appSupport: appSupport)
        let directory = CrashInbox.directory(dataDir: dataDir)
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [
                    .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication,
                ]
            )
        } catch {
            RsLog.marker("CRASH", ["source": "metrickit", "error": "dir"])
            return
        }

        var written = 0
        var failed = 0
        for (index, item) in items.enumerated() {
            guard let record = CrashInbox.fromMetricKit(
                json: item.0,
                incidentAtUs: item.1
            ), let data = CrashInbox.encode(record) else {
                failed += 1
                continue
            }

            let url = directory.appendingPathComponent(
                CrashInbox.fileName(incidentAtUs: item.1, index: index),
                isDirectory: false
            )
            do {
                try data.write(
                    to: url,
                    options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]
                )
                written += 1
            } catch {
                failed += 1
            }
        }

        if let names = try? fileManager.contentsOfDirectory(atPath: directory.path) {
            for name in CrashInbox.namesToDelete(names) {
                try? fileManager.removeItem(
                    at: directory.appendingPathComponent(name, isDirectory: false)
                )
            }
        }

        RsLog.marker("CRASH", [
            "source": "metrickit",
            "written": written,
            "failed": failed,
        ])
    }
}

// MXMetricManagerSubscriber 是 ObjC 协议：遵循它的类若对 ObjC 可见，会被导出到 RemoteScreen-Swift.h，
// 而导入该头文件的 .mm 没有引入 MetricKit，编译失败。所以订阅者放在 private 类里。
private final class MetricKitSubscriber: NSObject, MXMetricManagerSubscriber {
    private let onPayloads: ([MXDiagnosticPayload]) -> Void

    init(onPayloads: @escaping ([MXDiagnosticPayload]) -> Void) {
        self.onPayloads = onPayloads
        super.init()
    }

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
        onPayloads(payloads)
    }
}
