import Foundation

enum RsMarkerLine {
    static func format(tag: String, fields: [String: Any]) -> String {
        guard isValidTag(tag) else {
            let truncated = String(tag.prefix(32))
            return "RS-INVALID \(jsonString(["tag": truncated]))"
        }

        let normalized = fields.mapValues(normalize)
        return "RS-\(tag) \(jsonString(normalized))"
    }

    private static func isValidTag(_ tag: String) -> Bool {
        guard !tag.isEmpty, tag.utf8.count <= 32 else {
            return false
        }
        return tag.unicodeScalars.allSatisfy { scalar in
            (scalar.value >= 65 && scalar.value <= 90) ||
            (scalar.value >= 48 && scalar.value <= 57) ||
            scalar.value == 45
        }
    }

    private static func normalize(_ value: Any) -> Any {
        if let string = value as? String {
            return string
        }
        if let bool = value as? Bool {
            return bool
        }
        if let integer = value as? Int {
            return integer
        }
        if let double = value as? Double {
            return double.isFinite ? double : NSNull()
        }
        if let array = value as? [Any] {
            return array.map(normalize)
        }

        return "<unsupported>"
    }

    private static func jsonString(_ object: [String: Any]) -> String {
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(
                  withJSONObject: object,
                  options: [.sortedKeys]
              ),
              let string = String(data: data, encoding: .utf8) else {
            return "{}"
        }
        return string
    }
}
