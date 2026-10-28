import XCTest
@testable import NativeLogic

final class CrashTests: XCTestCase {
    func testFullDiagnostic() throws {
        let subFrame: [String: Any] = [
            "binaryName": "libsystem_kernel.dylib",
            "binaryUUID": "AAAAAAAA-1111-2222-3333-444455556666",
            "address": NSNumber(value: UInt64(7_000_000_000)),
            "offsetIntoBinaryTextSegment": NSNumber(value: UInt64(1_234)),
        ]
        let rootFrame: [String: Any] = [
            "binaryName": "RemoteScreen",
            "binaryUUID": "6B2E0C1A-1111-2222-3333-444455556666",
            "address": NSNumber(value: UInt64(4_372_377_360)),
            "offsetIntoBinaryTextSegment": NSNumber(value: UInt64(49_443)),
            "subFrames": [subFrame],
        ]
        let otherFrame: [String: Any] = ["binaryName": "Other"]
        let firstStack: [String: Any] = [
            "threadAttributed": false,
            "callStackRootFrames": [otherFrame],
        ]
        let secondStack: [String: Any] = [
            "threadAttributed": true,
            "callStackRootFrames": [rootFrame],
        ]
        let metadata: [String: Any] = [
            "appVersion": "0.1.0",
            "appBuildVersion": "7",
            "osVersion": "iPhone OS 17.4 (21E219)",
            "exceptionType": NSNumber(value: 1),
            "signal": NSNumber(value: 11),
            "exceptionCode": NSNumber(value: 0),
            "terminationReason": "Namespace SIGNAL, Code 11 Segmentation fault: 11",
            "virtualMemoryRegionInfo": "0x10 is not in any region.",
        ]
        let json = jsonData([
            "diagnosticMetaData": metadata,
            "callStackTree": [
                "callStackPerThread": true,
                "callStacks": [firstStack, secondStack],
            ],
        ])

        let record = try XCTUnwrap(
            CrashInbox.fromMetricKit(json: json, incidentAtUs: 1_792_000_000_000_000)
        )
        XCTAssertEqual(record.name, "EXC_BAD_ACCESS / SIGSEGV")
        XCTAssertEqual(
            record.message,
            "Namespace SIGNAL, Code 11 Segmentation fault: 11\n0x10 is not in any region."
        )
        XCTAssertEqual(
            record.stack,
            "0 RemoteScreen 0x1049d2f10 6B2E0C1A-1111-2222-3333-444455556666 +49443\n" +
                "1 libsystem_kernel.dylib 0x1a13b8600 AAAAAAAA-1111-2222-3333-444455556666 +1234"
        )
        XCTAssertEqual(record.appVersion, "0.1.0")
        XCTAssertEqual(record.osVersion, "iPhone OS 17.4 (21E219)")
        XCTAssertNil(record.thread)
        XCTAssertEqual(record.incidentAtUs, 1_792_000_000_000_000)
    }

    func testObjcExceptionWins() throws {
        let metadata: [String: Any] = [
            "objectiveCexceptionReason": [
                "exceptionName": "NSInvalidArgumentException",
                "composedMessage": "*** -[__NSArrayM insertObject:atIndex:]: object cannot be nil",
            ],
            "exceptionType": NSNumber(value: 10),
            "signal": NSNumber(value: 6),
            "terminationReason": "Namespace SIGNAL, Code 6 Abort trap: 6",
        ]
        let record = try XCTUnwrap(record(metadata: metadata))
        XCTAssertEqual(record.name, "NSInvalidArgumentException")
        XCTAssertEqual(
            record.message,
            "*** -[__NSArrayM insertObject:atIndex:]: object cannot be nil\n" +
                "Namespace SIGNAL, Code 6 Abort trap: 6"
        )
    }

    func testNames() throws {
        XCTAssertEqual(
            try XCTUnwrap(record(metadata: [
                "exceptionType": NSNumber(value: 99),
                "signal": NSNumber(value: 77),
            ])).name,
            "EXC_99 / SIG77"
        )
        XCTAssertEqual(
            try XCTUnwrap(record(metadata: ["signal": NSNumber(value: 6)])).name,
            "SIGABRT"
        )
        XCTAssertEqual(
            try XCTUnwrap(record(metadata: ["exceptionType": NSNumber(value: 12)])).name,
            "EXC_GUARD"
        )
        XCTAssertNil(record(metadata: [:])?.name)
        XCTAssertEqual(
            try XCTUnwrap(record(metadata: [
                "objectiveCexceptionReason": ["exceptionName": "  "],
                "exceptionType": NSNumber(value: 12),
            ])).name,
            "EXC_GUARD"
        )
    }

    func testEmptyDiagnostic() throws {
        let record = try XCTUnwrap(
            CrashInbox.fromMetricKit(json: jsonData([String: Any]()), incidentAtUs: 0)
        )
        XCTAssertEqual(record.message, "native crash")
        XCTAssertNil(record.name)
        XCTAssertNil(record.stack)
        XCTAssertNil(record.appVersion)
        XCTAssertNil(record.osVersion)
    }

    func testFrameLimit() throws {
        let root = try XCTUnwrap(nestedFrameRoot(count: 70))
        let record = try XCTUnwrap(record(metadata: [:], callStacks: [[
            "threadAttributed": true,
            "callStackRootFrames": [root],
        ]]))
        let lines = (try XCTUnwrap(record.stack))
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map(String.init)
        XCTAssertEqual(lines.count, 65)
        XCTAssertTrue(lines[0].hasPrefix("0 B0 "))
        XCTAssertTrue(lines[63].hasPrefix("63 B63 "))
        XCTAssertEqual(lines[64], "…")

        let exactRoot = try XCTUnwrap(nestedFrameRoot(count: 64))
        let exactRecord = try XCTUnwrap(self.record(metadata: [:], callStacks: [[
            "threadAttributed": true,
            "callStackRootFrames": [exactRoot],
        ]]))
        let exactLines = (try XCTUnwrap(exactRecord.stack))
            .split(separator: "\n")
            .map(String.init)
        XCTAssertEqual(exactLines.count, 64)
        XCTAssertFalse(exactLines.contains("…"))
    }

    func testSiblingFramesDepthFirst() throws {
        let a1: [String: Any] = ["binaryName": "A1"]
        let a: [String: Any] = ["binaryName": "A", "subFrames": [a1]]
        let root: [String: Any] = [
            "binaryName": "R",
            "subFrames": [a, ["binaryName": "B"]],
        ]
        let record = try XCTUnwrap(record(metadata: [:], callStacks: [[
            "callStackRootFrames": [root],
        ]]))
        let lines = (try XCTUnwrap(record.stack)).split(separator: "\n").map(String.init)
        XCTAssertEqual(
            lines,
            [
                "0 R 0x0 ? +0",
                "1 A 0x0 ? +0",
                "2 A1 0x0 ? +0",
                "3 B 0x0 ? +0",
            ]
        )
    }

    func testFallbackToFirstStack() throws {
        let first: [String: Any] = [
            "threadAttributed": false,
            "callStackRootFrames": [["binaryName": "First"]],
        ]
        let second: [String: Any] = [
            "threadAttributed": false,
            "callStackRootFrames": [["binaryName": "Second"]],
        ]
        let record = try XCTUnwrap(record(metadata: [:], callStacks: [first, second]))
        XCTAssertTrue((try XCTUnwrap(record.stack)).hasPrefix("0 First "))
    }

    func testMissingFrameFields() throws {
        let record = try XCTUnwrap(record(metadata: [:], callStacks: [[
            "callStackRootFrames": [[:]],
        ]]))
        XCTAssertEqual(record.stack, "0 ? 0x0 ? +0")
    }

    func testInvalidJson() {
        XCTAssertNil(CrashInbox.fromMetricKit(json: Data("not json".utf8), incidentAtUs: 0))
        XCTAssertNil(CrashInbox.fromMetricKit(json: jsonData([1]), incidentAtUs: 0))
    }

    func testEncodeExact() throws {
        let record = CrashNativeRecord(incidentAtUs: 5, message: "m")
        let data = try XCTUnwrap(CrashInbox.encode(record))
        XCTAssertEqual(
            String(data: data, encoding: .utf8),
            "{\"appVersion\":null,\"incidentAt\":5,\"message\":\"m\",\"name\":null,\"osVersion\":null,\"stack\":null,\"thread\":null,\"v\":1}"
        )
    }

    func testEncodeRoundTrip() throws {
        let record = CrashNativeRecord(
            incidentAtUs: 1_792_000_000_000_000,
            appVersion: "0.1.0",
            osVersion: "iPhone OS 17.4",
            name: "EXC_CRASH",
            message: "m",
            stack: "a/b",
            thread: "main"
        )
        let data = try XCTUnwrap(CrashInbox.encode(record))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(
            (object["incidentAt"] as? NSNumber)?.uint64Value,
            1_792_000_000_000_000
        )
        XCTAssertEqual(object["appVersion"] as? String, "0.1.0")
        XCTAssertEqual(object["osVersion"] as? String, "iPhone OS 17.4")
        XCTAssertEqual(object["name"] as? String, "EXC_CRASH")
        XCTAssertEqual(object["message"] as? String, "m")
        XCTAssertEqual(object["stack"] as? String, "a/b")
        XCTAssertEqual(object["thread"] as? String, "main")
        XCTAssertTrue(try XCTUnwrap(String(data: data, encoding: .utf8)).contains("a/b"))
    }

    func testFileName() {
        XCTAssertEqual(
            CrashInbox.fileName(incidentAtUs: 1_792_000_000_000_000, index: 2),
            "00001792000000000000-metrickit-2.json"
        )
    }

    func testNamesToDelete() {
        let names = [
            "00000000000000000003-metrickit-0.json",
            ".tmp",
            "notes.txt",
            "00000000000000000001-metrickit-0.json",
            "00000000000000000002-exitinfo-1.json",
        ]
        XCTAssertEqual(
            CrashInbox.namesToDelete(names, keep: 2),
            ["00000000000000000001-metrickit-0.json"]
        )
        XCTAssertEqual(CrashInbox.namesToDelete(names, keep: 10), [])
    }

    func testIncidentMicrosAndDirectory() {
        XCTAssertEqual(
            CrashInbox.incidentMicros(Date(timeIntervalSince1970: 1_792_000_000.5)),
            1_792_000_000_500_000
        )
        XCTAssertEqual(CrashInbox.incidentMicros(Date(timeIntervalSince1970: -1)), 0)
        XCTAssertEqual(
            CrashInbox.directory(dataDir: URL(fileURLWithPath: "/x/core")).path,
            "/x/core/crashes/native"
        )
    }

    private func jsonData(_ object: Any) -> Data {
        guard let data = try? JSONSerialization.data(withJSONObject: object) else {
            XCTFail("test fixture is not valid JSON")
            return Data()
        }
        return data
    }

    private func record(
        metadata: [String: Any],
        callStacks: [[String: Any]]? = nil
    ) -> CrashNativeRecord? {
        var object: [String: Any] = ["diagnosticMetaData": metadata]
        if let callStacks {
            object["callStackTree"] = [
                "callStackPerThread": true,
                "callStacks": callStacks,
            ]
        }
        return CrashInbox.fromMetricKit(json: jsonData(object), incidentAtUs: 0)
    }

    private func nestedFrameRoot(count: Int) -> [String: Any]? {
        guard count > 0 else {
            return nil
        }
        var next: [String: Any]?
        for index in stride(from: count - 1, through: 0, by: -1) {
            var frame: [String: Any] = ["binaryName": "B\(index)"]
            if let next {
                frame["subFrames"] = [next]
            }
            next = frame
        }
        return next
    }
}
