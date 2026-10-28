import Foundation
import Network

@objc(NetInfoMonitor)
final class NetInfoMonitor: NSObject {
    typealias ChangeHandler = (NetInfoSnapshot) -> Void

    @objc(shared)
    static let shared = NetInfoMonitor()

    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "rs.netinfo")
    private var vpnPools: [NetInfoCidr] = []
    private var lastSnapshot: NetInfoSnapshot?
    private var sequence: UInt64 = 0
    private var debouncer = NetInfoDebouncer()
    private var pendingWork: DispatchWorkItem?
    private var refreshGeneration = 0
    private var handler: ChangeHandler?
    private var started = false

    private override init() {}

    @objc(startWithCallback:)
    func startWithCallback(_ callback: @escaping (NSDictionary) -> Void) {
        start { [weak self] snapshot in
            guard let self else { return }
            callback(self.dictionary(for: snapshot))
        }
    }

    @objc(setVpnPools:)
    func setVpnPoolsForObjC(_ pools: [String]) {
        setVpnPools(pools)
    }

    @objc(status)
    func statusForObjC() -> NSDictionary {
        let snapshot = getStatus()
        return queue.sync { dictionary(for: snapshot, sequence: sequence) }
    }

    func start(handler: @escaping ChangeHandler) {
        queue.async { [weak self] in
            guard let self else { return }
            self.handler = handler
            guard !self.started else { return }
            self.started = true
            self.monitor.pathUpdateHandler = { [weak self] _ in
                self?.scheduleRefresh()
            }
            self.monitor.start(queue: self.queue)
            self.scheduleRefresh()
        }
    }

    func setVpnPools(_ pools: [String]) {
        queue.async { [weak self] in
            guard let self else { return }
            self.vpnPools = pools.compactMap(NetInfoCidr.parse)
            // A changed pool is configuration input rather than a path flap:
            // recalculate immediately and notify when the classified snapshot
            // changes.
            self.refreshAndNotify()
        }
    }

    func getStatus() -> NetInfoSnapshot {
        // getStatus is deliberately synchronous from the monitor's point of
        // view: it reads the current interfaces and path without waiting for
        // the trailing-edge debounce used by path callbacks.
        let raw = NetInfoInterfaces.read()
        let path = pathSnapshot(monitor.currentPath)
        let pools = queue.sync { vpnPools }
        return NetInfoClassifier.snapshot(raw, pools: pools).with(path: path)
    }

    func currentSequence() -> UInt64 {
        queue.sync { sequence }
    }

    private func scheduleRefresh() {
        pendingWork?.cancel()
        debouncer.update(at: DispatchTime.now().uptimeNanoseconds / 1_000)
        refreshGeneration &+= 1
        let generation = refreshGeneration
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            let now = DispatchTime.now().uptimeNanoseconds / 1_000
            self.debouncer.fireIfDue(at: now) {
                self.refreshAndNotify(generation: generation)
            }
        }
        pendingWork = work
        queue.asyncAfter(deadline: .now() + .milliseconds(500), execute: work)
    }

    private func refreshAndNotify(generation: Int? = nil) {
        if let generation, generation != refreshGeneration { return }
        let raw = NetInfoInterfaces.read()
        let snapshot = NetInfoClassifier.snapshot(raw, pools: vpnPools)
            .with(path: pathSnapshot(monitor.currentPath))
        guard snapshot != lastSnapshot else { return }
        lastSnapshot = snapshot
        sequence &+= 1
        let seq = sequence
        let handler = self.handler
        let ifaceNames = snapshot.ifaces.map(\.name)
        let vpnNames = snapshot.ifaces.filter(\.isVpn).map(\.name)
        let ifaceJson = ifaceNames.isEmpty ? "[]" : "[\"\(ifaceNames.joined(separator: "\",\""))\"]"
        let vpnJson = vpnNames.isEmpty ? "[]" : "[\"\(vpnNames.joined(separator: "\",\""))\"]"
        NSLog("RS-NETINFO {\"seq\":%llu,\"ifaces\":%@,\"vpn\":%@,\"path\":\"%@\"}",
              seq,
              ifaceJson,
              vpnJson,
              snapshot.path.interfaceTypes.first ?? snapshot.path.status)
        handler?(snapshot)
    }

    private func pathSnapshot(_ path: NWPath) -> NetInfoPath {
        let status: String
        switch path.status {
        case .satisfied: status = "satisfied"
        case .requiresConnection: status = "requiresConnection"
        case .unsatisfied: status = "unsatisfied"
        @unknown default: status = "unsatisfied"
        }
        let types: [(NWInterface.InterfaceType, String)] = [
            (.wifi, "wifi"),
            (.cellular, "cellular"),
            (.wiredEthernet, "wiredEthernet"),
            (.other, "other"),
        ]
        let interfaceTypes = types.compactMap { path.usesInterfaceType($0.0) ? $0.1 : nil }
        return NetInfoPath(
            status: status,
            interfaceTypes: interfaceTypes,
            expensive: path.isExpensive,
            constrained: path.isConstrained
        )
    }

    private func dictionary(for snapshot: NetInfoSnapshot, sequence: UInt64? = nil) -> NSDictionary {
        let ifaceValues: [[String: Any]] = snapshot.ifaces.map { iface in
            ["name": iface.name, "addrs": iface.addrs, "isVpn": iface.isVpn]
        }
        let path: [String: Any] = [
            "status": snapshot.path.status,
            "interfaceTypes": snapshot.path.interfaceTypes,
            "expensive": snapshot.path.expensive,
            "constrained": snapshot.path.constrained,
        ]
        return [
            "ifaces": ifaceValues,
            "path": path,
            "seq": NSNumber(value: Double(sequence ?? self.sequence)),
        ] as NSDictionary
    }
}

private extension NetInfoSnapshot {
    func with(path: NetInfoPath) -> NetInfoSnapshot {
        NetInfoSnapshot(ifaces: ifaces, path: path)
    }
}
