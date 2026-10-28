import Foundation

enum AdapterKeychainLogic {
    static let service = "ai.gxlself.remotescreen.adapter"
    static let maxKeyBytes = 300
    static let maxValueBytes = 4096
    static let installedFlagKey = "rs.adapter.secrets.installed.v1"

    // Mirror Security status constants without importing the framework.
    private static let itemNotFound: Int32 = -25300
    private static let duplicateItem: Int32 = -25299

    static func account(fromKeyBytes bytes: [UInt8]) -> String? {
        guard !bytes.isEmpty, bytes.count <= maxKeyBytes else { return nil }
        return String(bytes: bytes, encoding: .utf8)
    }

    static func getCode(status: Int32) -> Int32 {
        status == itemNotFound ? 1 : putCode(lastStatus: status)
    }

    static func fits(valueCount: Int, capacity: Int) -> Bool {
        valueCount >= 0 && valueCount <= capacity && valueCount <= maxValueBytes
    }

    static func deleteCode(status: Int32) -> Int32 {
        status == itemNotFound ? 0 : putCode(lastStatus: status)
    }

    enum PutStep: Equatable {
        case update, add, updateAgain
    }

    static func nextPutStep(after step: PutStep, status: Int32) -> PutStep? {
        switch step {
        case .update:
            return status == itemNotFound ? .add : nil
        case .add:
            return status == duplicateItem ? .updateAgain : nil
        case .updateAgain:
            return nil
        }
    }

    static func putCode(lastStatus: Int32) -> Int32 {
        lastStatus == 1 ? -1 : lastStatus
    }

    static func shouldPurge(installedFlag: Bool) -> Bool {
        !installedFlag
    }
}
