import Foundation

struct CrashNativeRecord: Equatable {
    var incidentAtUs: UInt64
    var appVersion: String? = nil
    var osVersion: String? = nil
    var name: String? = nil
    var message: String
    var stack: String? = nil
    var thread: String? = nil
}

enum CrashInbox {
    static let schemaVersion = 1
    static let maxFrames = 64
    static let keepFiles = 10

    static func directory(dataDir: URL) -> URL {
        dataDir
            .appendingPathComponent("crashes", isDirectory: true)
            .appendingPathComponent("native", isDirectory: true)
    }

    static func incidentMicros(_ date: Date) -> UInt64 {
        let us = date.timeIntervalSince1970 * 1_000_000
        guard us > 0 else {
            return 0
        }
        return UInt64(us)
    }

    static func exceptionName(_ code: Int) -> String {
        switch code {
        case 1:
            return "EXC_BAD_ACCESS"
        case 2:
            return "EXC_BAD_INSTRUCTION"
        case 3:
            return "EXC_ARITHMETIC"
        case 4:
            return "EXC_EMULATION"
        case 5:
            return "EXC_SOFTWARE"
        case 6:
            return "EXC_BREAKPOINT"
        case 10:
            return "EXC_CRASH"
        case 11:
            return "EXC_RESOURCE"
        case 12:
            return "EXC_GUARD"
        case 13:
            return "EXC_CORPSE_NOTIFY"
        default:
            return "EXC_\(code)"
        }
    }

    static func signalName(_ code: Int) -> String {
        switch code {
        case 1:
            return "SIGHUP"
        case 2:
            return "SIGINT"
        case 3:
            return "SIGQUIT"
        case 4:
            return "SIGILL"
        case 5:
            return "SIGTRAP"
        case 6:
            return "SIGABRT"
        case 7:
            return "SIGEMT"
        case 8:
            return "SIGFPE"
        case 9:
            return "SIGKILL"
        case 10:
            return "SIGBUS"
        case 11:
            return "SIGSEGV"
        case 12:
            return "SIGSYS"
        case 13:
            return "SIGPIPE"
        case 14:
            return "SIGALRM"
        case 15:
            return "SIGTERM"
        default:
            return "SIG\(code)"
        }
    }

    static func fromMetricKit(json: Data, incidentAtUs: UInt64) -> CrashNativeRecord? {
        guard let object = try? JSONSerialization.jsonObject(with: json),
              let dictionary = object as? [String: Any] else {
            return nil
        }

        let metadata = dictionary["diagnosticMetaData"] as? [String: Any] ?? [:]
        let objectiveCReason = metadata["objectiveCexceptionReason"] as? [String: Any]
        let appVersion = stringValue(metadata["appVersion"])
        let osVersion = stringValue(metadata["osVersion"])

        let name: String?
        if let objectiveCName = stringValue(objectiveCReason?["exceptionName"]) {
            name = objectiveCName
        } else {
            var names: [String] = []
            if let exceptionType = (metadata["exceptionType"] as? NSNumber)?.intValue {
                names.append(exceptionName(exceptionType))
            }
            if let signal = (metadata["signal"] as? NSNumber)?.intValue {
                names.append(signalName(signal))
            }
            name = names.isEmpty ? nil : names.joined(separator: " / ")
        }

        let message = [
            stringValue(objectiveCReason?["composedMessage"]),
            stringValue(metadata["terminationReason"]),
            stringValue(metadata["virtualMemoryRegionInfo"]),
        ]
        .compactMap { $0 }
        .joined(separator: "\n")

        let stack = stackValue(from: dictionary)
        return CrashNativeRecord(
            incidentAtUs: incidentAtUs,
            appVersion: appVersion,
            osVersion: osVersion,
            name: name,
            message: message.isEmpty ? "native crash" : message,
            stack: stack,
            thread: nil
        )
    }

    static func encode(_ record: CrashNativeRecord) -> Data? {
        let object: [String: Any] = [
            "v": schemaVersion,
            "incidentAt": NSNumber(value: record.incidentAtUs),
            "appVersion": record.appVersion ?? NSNull(),
            "osVersion": record.osVersion ?? NSNull(),
            "name": record.name ?? NSNull(),
            "message": record.message,
            "stack": record.stack ?? NSNull(),
            "thread": record.thread ?? NSNull(),
        ]

        do {
            return try JSONSerialization.data(
                withJSONObject: object,
                options: [.sortedKeys, .withoutEscapingSlashes]
            )
        } catch {
            return nil
        }
    }

    static func fileName(incidentAtUs: UInt64, index: Int) -> String {
        String(format: "%020llu-metrickit-%d.json", incidentAtUs, index)
    }

    static func namesToDelete(_ names: [String], keep: Int = keepFiles) -> [String] {
        let candidates = names
            .filter { $0.hasSuffix(".json") && !$0.hasPrefix(".") }
            .sorted()
        let filesToKeep = max(keep, 0)
        guard candidates.count > filesToKeep else {
            return []
        }
        return Array(candidates.prefix(candidates.count - filesToKeep))
    }

    private static func stringValue(_ value: Any?) -> String? {
        guard let value = value as? String else {
            return nil
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func stackValue(from dictionary: [String: Any]) -> String? {
        let stackTree = dictionary["callStackTree"] as? [String: Any]
        let stacks = stackTree?["callStacks"] as? [[String: Any]] ?? []
        guard let selectedStack = stacks.first(where: {
            ($0["threadAttributed"] as? Bool) == true
        }) ?? stacks.first,
              let rootFrames = selectedStack["callStackRootFrames"] as? [[String: Any]],
              !rootFrames.isEmpty else {
            return nil
        }

        var lines: [String] = []
        var truncated = false

        func visit(_ frame: [String: Any]) {
            guard lines.count < maxFrames else {
                truncated = true
                return
            }

            let index = lines.count
            let binaryName = stringValue(frame["binaryName"]) ?? "?"
            let binaryUUID = stringValue(frame["binaryUUID"]) ?? "?"
            let address = (frame["address"] as? NSNumber)?.uint64Value ?? 0
            let offset = (frame["offsetIntoBinaryTextSegment"] as? NSNumber)?.uint64Value ?? 0
            lines.append(
                "\(index) \(binaryName) 0x\(String(address, radix: 16)) \(binaryUUID) +\(offset)"
            )

            guard let subFrames = frame["subFrames"] as? [[String: Any]],
                  !subFrames.isEmpty else {
                return
            }

            if lines.count >= maxFrames {
                truncated = true
                return
            }
            for subFrame in subFrames {
                visit(subFrame)
                if truncated {
                    return
                }
            }
        }

        for rootFrame in rootFrames {
            visit(rootFrame)
            if truncated {
                break
            }
        }

        if truncated {
            lines.append("…")
        }
        return lines.joined(separator: "\n")
    }
}
