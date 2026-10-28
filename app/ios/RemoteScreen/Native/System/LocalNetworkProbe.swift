import Foundation
import Network

/// Runs the Bonjour probe used to make iOS present and report local-network
/// access. Calls made while a probe is running share its eventual result.
@objc(LocalNetworkProbe)
final class LocalNetworkProbe: NSObject {
    @objc(shared)
    static let shared = LocalNetworkProbe()

    private let queue = DispatchQueue(label: "rs.system.local-network")
    private var listener: NWListener?
    private var browser: NWBrowser?
    private var timeoutWork: DispatchWorkItem?
    private var pending: [(String) -> Void] = []
    private var state = LocalNetworkProbeState()
    private var startedAt: UInt64 = 0
    private var serviceName: String?

    private override init() {}

    @objc(probeWithTimeoutMs:completion:)
    func probe(timeoutMs: Double, completion: @escaping (String) -> Void) {
        let timeout = max(1, timeoutMs)
        queue.async { [weak self] in
            guard let self else { return }
            self.pending.append(completion)
            guard self.pending.count == 1 else { return }
            self.begin(timeoutMs: timeout)
        }
    }

    private func begin(timeoutMs: Double) {
        state = LocalNetworkProbeState()
        startedAt = DispatchTime.now().uptimeNanoseconds / 1_000_000
        let name = "rsp-\(UUID().uuidString.prefix(8))"
        serviceName = name

        do {
            let listener = try NWListener(using: .tcp)
            listener.service = NWListener.Service(
                name: name,
                type: "_rsp-probe._tcp"
            )
            listener.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                self.queue.async {
                    switch state {
                    case .failed:
                        self.finishIfNeeded(event: .listenerFailed)
                    default:
                        break
                    }
                }
            }
            listener.newConnectionHandler = { connection in
                connection.cancel()
            }
            self.listener = listener
            listener.start(queue: queue)
        } catch {
            finishIfNeeded(event: .listenerFailed)
            return
        }

        let browser = NWBrowser(
            for: .bonjour(type: "_rsp-probe._tcp", domain: nil),
            using: .tcp
        )
        browser.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            self.queue.async {
                switch state {
                case .waiting(let error):
                    let code: Int
                    if case .dns(let dnsCode) = error {
                        code = Int(dnsCode)
                    } else {
                        code = -1
                    }
                    self.finishIfNeeded(event: .waiting(code))
                case .failed:
                    // Browser failure does not prove that local-network
                    // access was denied; the timeout gives the final answer.
                    break
                default:
                    break
                }
            }
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            guard let self else { return }
            self.queue.async {
                let ownName = self.serviceName
                for result in results {
                    guard case .service(let name, _, _, _) = result.endpoint else {
                        continue
                    }
                    if name == ownName {
                        self.finishIfNeeded(event: .foundSelf)
                        return
                    }
                    self.finishIfNeeded(event: .foundOther)
                }
            }
        }
        self.browser = browser
        browser.start(queue: queue)

        let timeoutWork = DispatchWorkItem { [weak self] in
            self?.finishIfNeeded(event: .timeout)
        }
        self.timeoutWork = timeoutWork
        queue.asyncAfter(
            deadline: .now() + .milliseconds(Int(timeoutMs.rounded())),
            execute: timeoutWork
        )
    }

    private func finishIfNeeded(event: LocalNetworkProbeState.Event) {
        guard !pending.isEmpty else { return }
        _ = state.receive(event)
        guard let result = state.result else { return }
        finish(result: result)
    }

    private func finish(result: String) {
        guard !pending.isEmpty else { return }
        let elapsed = (DispatchTime.now().uptimeNanoseconds / 1_000_000) - startedAt
        timeoutWork?.cancel()
        timeoutWork = nil
        listener?.cancel()
        browser?.cancel()
        listener = nil
        browser = nil
        serviceName = nil
        let callbacks = pending
        pending.removeAll(keepingCapacity: false)
        RsLog.marker("SYSTEM", [
            "localNetwork": result,
            "ms": Int(elapsed),
        ])
        callbacks.forEach { $0(result) }
    }
}
