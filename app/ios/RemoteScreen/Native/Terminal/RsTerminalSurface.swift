import GameController
import SwiftTerm
import UIKit

@objc(RsTerminalSurface)
final class RsTerminalSurface: UIView, @preconcurrency TerminalViewDelegate {
    @objc var onModifiersConsumed: ((NSDictionary) -> Void)?
    @objc var onSizeChange: ((NSDictionary) -> Void)?
    @objc var onKeyboardFrame: ((NSDictionary) -> Void)?
    @objc var onActiveChange: ((NSDictionary) -> Void)?
    @objc var onCopied: ((NSDictionary) -> Void)?

    private let terminalView: RsSwiftTermView
    private let attachment = TerminalAttachment()
    private lazy var inputSender = TerminalInputSender(surface: self)
    private var sshIdValue = ""
    private var attachedId: UInt64?
    private var debugEcho = false
    private var attachmentEnabled = true
    private var suppliedModifiers: Int32 = 0
    private var currentModifiers: Int32 = 0
    private var suppliedKeyboardActive = false
    private var wantsKeyboard = false
    private var changingResponder = false
    private var keyboardVisible = false
    private var publishedActive = false

    override init(frame: CGRect) {
        let options = TerminalOptions(cols: 80, rows: 25, cursorStyle: .blinkBlock, scrollback: 500)
        terminalView = RsSwiftTermView(frame: .zero, font: Self.font(size: 13), options: options)
        super.init(frame: frame)
        isAccessibilityElement = false
        clipsToBounds = true
        inputSender.setDestination(sshId: nil, debugEcho: false)
        terminalView.terminalDelegate = self
        // Inset the view itself: SwiftTerm derives its grid from bounds and
        // does not subtract UIScrollView.contentInset when calculating columns.
        terminalView.contentInset = .zero
        terminalView.contentInsetAdjustmentBehavior = .never
        terminalView.autocapitalizationType = .none
        terminalView.autocorrectionType = .no
        terminalView.spellCheckingType = .no
        terminalView.smartQuotesType = .no
        terminalView.smartDashesType = .no
        terminalView.smartInsertDeleteType = .no
        terminalView.inputAccessoryView = nil
        terminalView.inputAssistantItem.leadingBarButtonGroups = []
        terminalView.inputAssistantItem.trailingBarButtonGroups = []
        terminalView.onResponderChange = { [weak self] active in
            guard let self, !self.changingResponder else { return }
            self.wantsKeyboard = active
            self.publishActive(active)
        }
        terminalView.onPaste = { [weak self] in self?.paste() }
        terminalView.onCopy = { [weak self] length in self?.onCopied?(["length": length]) }
        addSubview(terminalView)
        applyTheme()
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(keyboardFrameChanged(_:)),
                           name: UIResponder.keyboardWillChangeFrameNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardWillHide(_:)),
                           name: UIResponder.keyboardWillHideNotification, object: nil)
    }

    required init?(coder: NSCoder) { return nil }

    deinit {
        NotificationCenter.default.removeObserver(self)
        // Fabric owns and disposes UIView instances on the main queue.
        MainActor.assumeIsolated {
            inputSender.invalidate()
            attachment.detach()
            terminalView.updateUiClosed()
        }
    }

    @objc(configureSshId:fontSize:theme:modifiers:keyboardActive:debugEcho:)
    func configure(sshId value: String, fontSize: Float, theme: String,
                   modifiers: Int32, keyboardActive: Bool, debugEcho: Bool) {
        let nextFontSize = CGFloat(TerminalFontSize.clamp(Double(fontSize)))
        if abs(terminalView.font.pointSize - nextFontSize) > 0.01 {
            terminalView.font = Self.font(size: nextFontSize)
        }
        self.debugEcho = debugEcho
        if sshIdValue != value {
            disconnect()
            sshIdValue = value
            clearTerminal()
        }
        if suppliedModifiers != modifiers {
            suppliedModifiers = modifiers
            currentModifiers = modifiers & 0x03
        }
        if suppliedKeyboardActive != keyboardActive {
            suppliedKeyboardActive = keyboardActive
            wantsKeyboard = keyboardActive
        }
        // There is only one supported theme; it is installed at creation/reset.
        _ = theme
        updateInputDestination()
        if attachmentEnabled { attachment.attach(sshIdValue, surface: self) }
        syncResponder()
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        if newWindow == nil {
            attachmentEnabled = false
            disconnect()
            changingResponder = true
            _ = terminalView.resignFirstResponder()
            changingResponder = false
            publishActive(false)
        }
        super.willMove(toWindow: newWindow)
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            attachmentEnabled = true
            updateInputDestination()
            attachment.attach(sshIdValue, surface: self)
            syncResponder()
        }
    }

    @objc func reset() {
        disconnect()
        sshIdValue = ""
        debugEcho = false
        suppliedModifiers = 0
        currentModifiers = 0
        suppliedKeyboardActive = false
        wantsKeyboard = false
        changingResponder = true
        _ = terminalView.resignFirstResponder()
        changingResponder = false
        publishedActive = false
        keyboardVisible = false
        clearTerminal()
    }

    private func disconnect() {
        inputSender.invalidate()
        attachment.detach()
        attachedId = nil
    }

    private func clearTerminal() {
        terminalView.selectNone()
        terminalView.getTerminal().resetToInitialState()
        applyTheme()
    }

    private func updateInputDestination() {
        inputSender.setDestination(sshId: attachedId,
                                   debugEcho: attachmentEnabled && sshIdValue.isEmpty && debugEcho)
    }

    func terminalDidAttach(_ id: UInt64) {
        attachedId = id
        updateInputDestination()
        let terminal = terminalView.getTerminal()
        attachment.resize(cols: terminal.cols, rows: terminal.rows)
    }

    @objc func sendSequence(_ text: String) { inputSender.sendSequence(text) }
    @objc func sendKey(_ name: String) { inputSender.sendKey(name, modifiers: currentModifiers) }

    @objc func copySelection() {
        guard let text = terminalView.getSelection(), !text.isEmpty else { return }
        UIPasteboard.general.string = text
        onCopied?(["length": text.utf16.count])
    }

    @objc func paste() {
        guard let text = UIPasteboard.general.string else { return }
        let bytes = TerminalPasteEncoder.encode(text: text, bracketed: terminalView.getTerminal().bracketedPasteMode)
        inputSender.sendRaw(bytes)
    }

    func feedOutput(_ data: Data) {
        dispatchPrecondition(condition: .onQueue(.main))
        terminalView.feed(byteArray: Array(data)[...])
    }

    func feedLocal(_ bytes: [UInt8]) {
        dispatchPrecondition(condition: .onQueue(.main))
        terminalView.feed(byteArray: bytes[...])
    }

    func modifiersConsumed(_ mods: Int32) {
        currentModifiers = 0
        onModifiersConsumed?(["mods": mods])
    }

    var applicationCursor: Bool { terminalView.getTerminal().applicationCursor }

    private static func font(size: CGFloat) -> UIFont {
        UIFont(name: "GeistMono-Regular", size: size)
            ?? UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    private func applyTheme() {
        let theme = TerminalTheme.night
        func color(_ rgb: TerminalRGB) -> UIColor {
            UIColor(red: CGFloat(rgb.red) / 255, green: CGFloat(rgb.green) / 255,
                    blue: CGFloat(rgb.blue) / 255, alpha: 1)
        }
        backgroundColor = color(theme.background)
        terminalView.nativeForegroundColor = color(theme.foreground)
        terminalView.nativeBackgroundColor = color(theme.background)
        terminalView.caretColor = color(theme.cursor)
        terminalView.caretTextColor = color(theme.background)
        terminalView.installColors(theme.ansi.map {
            SwiftTerm.Color(red8: UInt16($0.red), green8: UInt16($0.green), blue8: UInt16($0.blue))
        })
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        terminalView.frame = CGRect(x: bounds.minX + 12, y: bounds.minY + 6,
                                    width: max(0, bounds.width - 24), height: max(0, bounds.height - 14))
    }

    private func syncResponder() {
        changingResponder = true
        if wantsKeyboard, window != nil {
            if !terminalView.isFirstResponder { _ = terminalView.becomeFirstResponder() }
        } else if terminalView.isFirstResponder {
            _ = terminalView.resignFirstResponder()
        }
        changingResponder = false
        publishActive(terminalView.isFirstResponder)
    }

    private func publishActive(_ active: Bool) {
        guard publishedActive != active else { return }
        publishedActive = active
        onActiveChange?(["active": active])
    }

    @objc private func keyboardFrameChanged(_ notification: Notification) {
        guard let window,
              terminalView.isFirstResponder || wantsKeyboard || keyboardVisible,
              let frame = notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue else { return }
        let keyboardFrame = window.convert(frame.cgRectValue, from: window.screen.coordinateSpace)
        let overlap = window.bounds.intersection(keyboardFrame)
        let touchesBottom = keyboardFrame.maxY >= window.bounds.maxY - 1
        let height = !overlap.isNull && touchesBottom ? max(0, overlap.height) : 0
        publishKeyboardFrame(height: height)
    }

    @objc private func keyboardWillHide(_ notification: Notification) {
        guard window != nil, terminalView.isFirstResponder || keyboardVisible || changingResponder else { return }
        publishKeyboardFrame(height: 0)
        // iPad can hide its software keyboard while retaining the responder.
        // A hardware keyboard must retain focus for pressesBegan to keep working.
        if !changingResponder, terminalView.isFirstResponder, GCKeyboard.coalesced == nil,
           UIApplication.shared.applicationState == .active {
            wantsKeyboard = false
            changingResponder = true
            _ = terminalView.resignFirstResponder()
            changingResponder = false
            publishActive(false)
        }
    }

    private func publishKeyboardFrame(height: CGFloat) {
        keyboardVisible = height > 0
        onKeyboardFrame?(["height": Double(height), "visible": keyboardVisible,
                          "hardware": GCKeyboard.coalesced != nil && height < 100])
    }

    // MARK: TerminalViewDelegate
    func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {
        guard newCols > 0, newRows > 0 else { return }
        attachment.resize(cols: newCols, rows: newRows)
        onSizeChange?(["cols": newCols, "rows": newRows])
    }

    func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) {
        inputSender.send(sourceData: data, modifiers: currentModifiers)
    }

    func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {}
    func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}
    func scrolled(source: SwiftTerm.TerminalView, position: Double) {}
    func requestOpenLink(source: SwiftTerm.TerminalView, link: String, params: [String: String]) {}
    func bell(source: SwiftTerm.TerminalView) {}
    func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {}
    func clipboardRead(source: SwiftTerm.TerminalView) -> Data? { nil }
    func iTermContent(source: SwiftTerm.TerminalView, content: ArraySlice<UInt8>) {}
    func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
}

private final class RsSwiftTermView: SwiftTerm.TerminalView {
    // SwiftTerm 1.20 exposes a non-open, read-only Swift keyboardType getter.
    // UIKit queries this Objective-C selector, which this subclass supplies
    // without changing the vendored dependency or hardware-key handling.
    @objc(keyboardType) var asciiKeyboardType: UIKeyboardType { .asciiCapable }

    var onResponderChange: ((Bool) -> Void)?
    var onPaste: (() -> Void)?
    var onCopy: ((Int) -> Void)?

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result { onResponderChange?(true) }
        return result
    }

    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result { onResponderChange?(false) }
        return result
    }

    override func paste(_ sender: Any?) {
        selectNone()
        onPaste?()
    }

    override func copy(_ sender: Any?) {
        guard let text = getSelection(), !text.isEmpty else { return }
        super.copy(sender)
        onCopy?(text.utf16.count)
    }
}
