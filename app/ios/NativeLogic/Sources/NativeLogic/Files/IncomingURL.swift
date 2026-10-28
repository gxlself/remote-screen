import Foundation

public enum IncomingURL {
    public static func isFile(_ url: URL) -> Bool {
        url.scheme?.lowercased() == "file"
    }
}
