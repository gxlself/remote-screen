import Foundation

public enum TerminalPasteEncoder {
    public static func encode(text: String, bracketed: Bool) -> [UInt8] {
        let withoutEsc = text.replacingOccurrences(of: "\u{1b}", with: "")
        let normalized = withoutEsc.replacingOccurrences(of: "\r\n", with: "\r")
            .replacingOccurrences(of: "\n", with: "\r")
        let body = Array(normalized.utf8)
        guard bracketed else { return body }
        return Array("\u{1b}[200~".utf8) + body + Array("\u{1b}[201~".utf8)
    }
}
