import Foundation

struct NetInfoPath: Equatable, Hashable {
    let status: String
    let interfaceTypes: [String]
    let expensive: Bool
    let constrained: Bool

    static let unknown = NetInfoPath(
        status: "unsatisfied",
        interfaceTypes: [],
        expensive: false,
        constrained: false
    )
}

struct NetInfoSnapshot: Equatable {
    let ifaces: [NetInfoIface]
    let path: NetInfoPath

    init(ifaces: [NetInfoIface], path: NetInfoPath = .unknown) {
        self.ifaces = ifaces.sorted { $0.name < $1.name }
        self.path = path
    }
}
