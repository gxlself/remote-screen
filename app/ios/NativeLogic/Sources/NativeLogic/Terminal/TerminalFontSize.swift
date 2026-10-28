public enum TerminalFontSize {
    public static func clamp(_ value: Double) -> Double {
        guard value.isFinite else { return 13 }
        return min(24, max(9, value))
    }
}
