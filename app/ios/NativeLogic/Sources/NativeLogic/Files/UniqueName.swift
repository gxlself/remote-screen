import Foundation

public enum UniqueName {
    /// Returns `name` when it is available, otherwise appends a numbered
    /// suffix before the extension until the result is available.
    public static func next(name: String, taken: Set<String>) -> String {
        guard taken.contains(name) else {
            return name
        }

        let (stem, suffix) = split(name: name)
        var index = 1
        while true {
            let candidate = "\(stem) (\(index))\(suffix)"
            if !taken.contains(candidate) {
                return candidate
            }
            index += 1
        }
    }

    private static func split(name: String) -> (stem: String, suffix: String) {
        let pathExtension = (name as NSString).pathExtension
        guard !pathExtension.isEmpty,
              !(name.hasPrefix(".") && !name.dropFirst().contains(".")) else {
            return (name, "")
        }

        let suffixLength = pathExtension.count + 1
        let stemEnd = name.index(name.endIndex, offsetBy: -suffixLength)
        return (String(name[..<stemEnd]), ".\(pathExtension)")
    }
}
