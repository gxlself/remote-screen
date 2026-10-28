/// When the invisible capture view should hold the keyboard focus.
enum KeyboardFocusPolicy {
    /// - Parameters:
    ///   - wanted: JS asked for the on-screen keyboard bar.
    ///   - hardware: a hardware keyboard is attached; it captures on its own.
    ///   - othersEditing: another text field on screen (a credential sheet,
    ///     a rename box) is being typed into. Hardware capture waits for it
    ///     and takes the focus back once it is done.
    static func shouldCapture(configured: Bool, appActive: Bool, inWindow: Bool,
                              wanted: Bool, hardware: Bool, othersEditing: Bool) -> Bool {
        guard configured, appActive, inWindow else { return false }
        return wanted || (hardware && !othersEditing)
    }
}
