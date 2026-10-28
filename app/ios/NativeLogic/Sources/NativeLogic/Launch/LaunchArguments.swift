import Foundation

enum LaunchArguments {
    private static let valueArguments = [
        "-autoConnect",
        "-devHarness",
        "-harnessArg",
        "-disconnectAfter",
        "-selfTestMove",
    ]

    static func properties(_ args: [String]) -> [String: String] {
        var result: [String: String] = [:]
        for argument in valueArguments {
            guard let value = value(for: argument, in: args) else {
                continue
            }
            result[String(argument.dropFirst())] = value
        }
        return result
    }

    private static func value(for argument: String, in args: [String]) -> String? {
        guard let index = args.firstIndex(of: argument), index + 1 < args.count else {
            return nil
        }

        let value = args[index + 1]
        guard !valueArguments.contains(value) else {
            return nil
        }
        return value
    }
}

enum LaunchDataDir {
    static func url(appSupport: URL) -> URL {
        appSupport
            .appendingPathComponent("RemoteScreen", isDirectory: true)
            .appendingPathComponent("core", isDirectory: true)
    }
}
