import Foundation
import Network
import UIKit

/// Browses Mac screen-sharing services and resolves the endpoint selected by
/// Network.framework. All mutable state is owned by `queue`; only event
/// delivery crosses to the main thread.
@objc(RsBonjourService)
final class RsBonjourService: NSObject {
    @objc(shared)
    static let shared = RsBonjourService()

    private struct Service {
        let name: String
        let host: String
        let port: Int
        let addrs: [String]

        var dictionary: NSDictionary {
            [
                "name": name,
                "host": host,
                "port": NSNumber(value: port),
                "addrs": addrs,
            ] as NSDictionary
        }
    }

    private struct Resolution {
        let connection: NWConnection
        let timeout: DispatchWorkItem
    }

    private let queue = DispatchQueue(label: "rs.system.bonjour")
    private var browser: NWBrowser?
    private var resultsByName: [String: Set<NWBrowser.Result>] = [:]
    private var resolutions: [UUID: Resolution] = [:]
    private var resolutionIDsByName: [String: UUID] = [:]
    private var services: [String: Service] = [:]
    private var discoveryState = BonjourDiscoveryState()
    private var generation: UInt64 = 0
    private var eventHandler: ((NSDictionary) -> Void)?
    private var backgroundObserver: NSObjectProtocol?

    private override init() {
        super.init()
        backgroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            self?.queue.async { [weak self] in
                self?.stopOnQueue(reason: "background")
            }
        }
    }

    deinit {
        if let backgroundObserver {
            NotificationCenter.default.removeObserver(backgroundObserver)
        }
    }

    @objc(registerWithCallback:)
    func registerWithCallback(_ callback: @escaping (NSDictionary) -> Void) {
        queue.sync {
            eventHandler = callback
        }
    }

    @objc(clearCallback)
    func clearCallback() {
        queue.sync {
            eventHandler = nil
        }
    }

    @objc(start:)
    func start(_ type: String) {
        queue.async { [weak self] in
            guard let self else { return }
            guard type == "_rfb._tcp" else {
                RsLog.marker("BONJOUR", ["state": "ignored", "reason": "type"])
                return
            }
            self.stopOnQueue(reason: "restart")
            self.generation &+= 1
            let currentGeneration = self.generation
            let browser = NWBrowser(
                for: .bonjour(type: "_rfb._tcp", domain: nil),
                using: .tcp
            )
            browser.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.queue.async {
                    guard self.generation == currentGeneration else { return }
                    switch state {
                    case .ready:
                        RsLog.marker("BONJOUR", ["state": "ready"])
                    case .waiting(_):
                        RsLog.marker("BONJOUR", ["state": "waiting"])
                    case .failed(_):
                        RsLog.marker("BONJOUR", ["state": "failed"])
                    case .cancelled:
                        break
                    case .setup:
                        break
                    @unknown default:
                        RsLog.marker("BONJOUR", ["state": "unknown"])
                    }
                }
            }
            browser.browseResultsChangedHandler = { [weak self] newResults, _ in
                guard let self else { return }
                self.queue.async {
                    guard self.generation == currentGeneration else { return }
                    self.handleResults(newResults)
                }
            }
            self.browser = browser
            browser.start(queue: self.queue)
            RsLog.marker("BONJOUR", ["state": "start"])
        }
    }

    @objc(stop)
    func stop() {
        queue.async { [weak self] in
            self?.stopOnQueue(reason: "stop")
        }
    }

    private func stopOnQueue(reason: String) {
        dispatchPrecondition(condition: .onQueue(queue))
        generation &+= 1
        browser?.cancel()
        browser = nil
        resultsByName.removeAll(keepingCapacity: false)
        for resolution in resolutions.values {
            resolution.timeout.cancel()
            resolution.connection.cancel()
        }
        resolutions.removeAll(keepingCapacity: false)
        resolutionIDsByName.removeAll(keepingCapacity: false)
        services.removeAll(keepingCapacity: false)
        discoveryState.reset()
        RsLog.marker("BONJOUR", ["state": reason])
    }

    private func handleResults(_ newResults: Set<NWBrowser.Result>) {
        var grouped: [String: Set<NWBrowser.Result>] = [:]
        for result in newResults {
            guard let name = serviceName(from: result) else { continue }
            grouped[name, default: []].insert(result)
        }

        let oldNames = Set(resultsByName.keys)
        let newNames = Set(grouped.keys)
        for name in oldNames.subtracting(newNames) {
            remove(name: name)
        }
        for name in newNames {
            guard let results = grouped[name] else { continue }
            resultsByName[name] = results
            guard !oldNames.contains(name),
                  discoveryState.found(name),
                  let result = results.first else {
                continue
            }
            let service = Service(
                name: name,
                host: BonjourLogic.host(for: name),
                port: 0,
                addrs: []
            )
            services[name] = service
            beginResolution(for: name, endpoint: result.endpoint)
        }
    }

    private func serviceName(from result: NWBrowser.Result) -> String? {
        guard case let .service(name, _, _, _) = result.endpoint, !name.isEmpty else {
            return nil
        }
        return name
    }

    private func remove(name: String) {
        guard resultsByName.removeValue(forKey: name) != nil else {
            return
        }
        guard discoveryState.lost(name) else { return }
        if let id = resolutionIDsByName.removeValue(forKey: name) {
            cancelResolution(id)
        }
        let service = services.removeValue(forKey: name)
            ?? Service(name: name, host: BonjourLogic.host(for: name), port: 0, addrs: [])
        RsLog.marker("BONJOUR", ["state": "lost"])
        emit(kind: "lost", service: service)
    }

    private func beginResolution(for name: String, endpoint: NWEndpoint) {
        let connection = NWConnection(to: endpoint, using: .tcp)
        let id = UUID()
        let currentGeneration = generation
        let timeout = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.finishResolution(
                id: id,
                generation: currentGeneration,
                name: name,
                endpoint: nil
            )
        }
        resolutions[id] = Resolution(connection: connection, timeout: timeout)
        resolutionIDsByName[name] = id
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self else { return }
            self.queue.async {
                guard self.generation == currentGeneration else { return }
                switch state {
                case .ready:
                    self.finishResolution(
                        id: id,
                        generation: currentGeneration,
                        name: name,
                        endpoint: connection?.currentPath?.remoteEndpoint
                    )
                case .failed(_), .cancelled:
                    self.finishResolution(
                        id: id,
                        generation: currentGeneration,
                        name: name,
                        endpoint: nil
                    )
                case .setup, .preparing, .waiting(_):
                    break
                @unknown default:
                    break
                }
            }
        }
        connection.start(queue: queue)
        queue.asyncAfter(deadline: .now() + .seconds(2), execute: timeout)
    }

    private func cancelResolution(_ id: UUID) {
        guard let resolution = resolutions.removeValue(forKey: id) else { return }
        resolution.timeout.cancel()
        resolution.connection.cancel()
    }

    private func finishResolution(
        id: UUID,
        generation: UInt64,
        name: String,
        endpoint: NWEndpoint?
    ) {
        guard self.generation == generation,
              let resolution = resolutions.removeValue(forKey: id) else {
            return
        }
        resolution.timeout.cancel()
        resolution.connection.cancel()
        if resolutionIDsByName[name] == id {
            resolutionIDsByName.removeValue(forKey: name)
        }
        guard discoveryState.names.contains(name) else { return }

        let parsed = parse(endpoint: endpoint)
        let service = Service(
            name: name,
            host: BonjourLogic.host(for: name),
            port: parsed.port,
            addrs: BonjourLogic.orderedAddresses(parsed.addresses)
        )
        services[name] = service
        RsLog.marker("BONJOUR", ["state": "found", "addrs": service.addrs.count])
        emit(kind: "found", service: service)
    }

    private func parse(endpoint: NWEndpoint?) -> (port: Int, addresses: [String]) {
        guard case let .hostPort(host, port) = endpoint else {
            return (0, [])
        }
        let address: String?
        switch host {
        case let .ipv4(value):
            address = value.debugDescription
        case let .ipv6(value):
            address = value.debugDescription
        case .name(_, _):
            address = nil
        }
        return (Int(port.rawValue), address.map { [$0] } ?? [])
    }

    private func emit(kind: String, service: Service) {
        let event: NSDictionary = [
            "kind": kind,
            "service": service.dictionary,
        ]
        let eventGeneration = generation
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let callback: ((NSDictionary) -> Void)? = self.queue.sync {
                guard self.generation == eventGeneration else { return nil }
                return self.eventHandler
            }
            callback?(event)
        }
    }
}
