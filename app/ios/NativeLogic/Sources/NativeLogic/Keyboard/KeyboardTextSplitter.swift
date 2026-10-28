enum KeyboardTextSplitter {
    /// Iterating Characters preserves extended grapheme clusters, including
    /// ZWJ emoji. A single cluster larger than the limit cannot be represented
    /// by the Text contract and is omitted rather than sent as an invalid frame.
    static func split(_ text: String, maxBytes: Int = 4096) -> [String] {
        guard maxBytes > 0 else { return [] }
        var parts: [String] = []
        var part = ""
        var bytes = 0
        for character in text {
            let count = character.utf8.count
            guard count <= maxBytes else {
                if !part.isEmpty { parts.append(part) }
                part = ""
                bytes = 0
                continue
            }
            if bytes + count > maxBytes {
                parts.append(part)
                part = ""
                bytes = 0
            }
            part.append(character)
            bytes += count
        }
        if !part.isEmpty { parts.append(part) }
        return parts
    }
}
