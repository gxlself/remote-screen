import UIKit

@objc(RsKeyCaptureView)
final class RsKeyCaptureView: UIView, UITextViewDelegate, UIGestureRecognizerDelegate {
    @objc var onKey: ((NSDictionary) -> Void)?
    @objc var onText: ((NSDictionary) -> Void)?
    @objc var onComposition: ((NSDictionary) -> Void)?
    @objc var onStickyConsumed: ((NSDictionary) -> Void)?
    @objc var onKeyboardFrame: ((NSDictionary) -> Void)?
    @objc var onActiveChange: ((NSDictionary) -> Void)?
    @objc var onPaste: ((NSDictionary) -> Void)?
    /// ⌃⌥⌘ session shortcuts (next / previous / Nth / switcher), kept on this device.
    @objc var onShortcut: ((NSDictionary) -> Void)? {
        didSet { connectShortcuts() }
    }
    @objc var syncPaste: Bool {
        get { textView.syncPaste }
        set { textView.syncPaste = newValue }
    }

    private let textView = RsKeyCaptureTextView(frame: CGRect(x: 0, y: 0, width: 1, height: 1))
    private let hardwareKeyboard = HardwareKeyboardMonitor()
    private var mode: KeyboardInputMode = .text
    private var os: KeyboardOS = .macos
    private var suppliedStickyMods: UInt8 = 0
    private var stickyMods: UInt8 = 0
    private var suppliedActive = false
    private var wantsFirstResponder = false
    private var changingResponder = false
    private var clearingText = false
    private var composition = ""
    private var keyboardVisible = false
    private var configured = false
    private var automaticallyCapturingHardware = false
    private var appActive = UIApplication.shared.applicationState == .active
    private struct KeyboardFrame: Equatable {
        let height: CGFloat
        let visible: Bool
        let hardware: Bool
    }
    private var lastKeyboardFrame: KeyboardFrame?
    /// Other text inputs being typed into (credential sheet, rename box).
    private var othersEditing: Set<ObjectIdentifier> = []
    private lazy var pictureTap: UITapGestureRecognizer = {
        let tap = UITapGestureRecognizer(target: self, action: #selector(pictureTapped(_:)))
        tap.cancelsTouchesInView = false
        tap.delaysTouchesBegan = false
        tap.delaysTouchesEnded = false
        tap.delegate = self
        return tap
    }()

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .clear
        isAccessibilityElement = false
        accessibilityElementsHidden = true

        textView.alpha = 0.02
        textView.backgroundColor = .clear
        textView.textColor = .clear
        textView.tintColor = .clear
        textView.isScrollEnabled = false
        textView.textContainerInset = .zero
        textView.textContainer.lineFragmentPadding = 0
        textView.isAccessibilityElement = false
        textView.accessibilityElementsHidden = true
        textView.inputAccessoryView = nil
        textView.inputAssistantItem.leadingBarButtonGroups = []
        textView.inputAssistantItem.trailingBarButtonGroups = []
        textView.delegate = self
        textView.onRawKey = { [weak self] usage, down, mods in
            self?.sendKey(usage: usage, down: down, mods: mods)
        }
        textView.onPaste = { [weak self] held in
            guard let self else { return }
            let mapped = KeyboardOsMap.remap(usage: 0, mods: held, os: self.os)
            self.onPaste?(["mods": Int(mapped.1)])
        }
        applyKeyboardTraits()
        connectShortcuts()
        addSubview(textView)
        hardwareKeyboard.onChange = { [weak self] connected in
            guard let self else { return }
            if !connected {
                self.textView.stopRepeat(.keyboardDisconnected)
                self.textView.releasePressedKeys()
            }
            self.syncResponder()
            if self.window != nil, self.configured {
                self.publishFrame(height: self.lastKeyboardFrame?.height ?? 0)
            }
        }

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(keyboardFrameChanged(_:)),
                           name: UIResponder.keyboardWillChangeFrameNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardWillHide(_:)),
                           name: UIResponder.keyboardWillHideNotification, object: nil)
        center.addObserver(self, selector: #selector(willResignActive),
                           name: UIApplication.willResignActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(didBecomeActive),
                           name: UIApplication.didBecomeActiveNotification, object: nil)
        // another window of the app may stay active; only this
        // view's own scene decides whether it holds the keyboard.
        center.addObserver(self, selector: #selector(sceneWillDeactivate(_:)),
                           name: UIScene.willDeactivateNotification, object: nil)
        center.addObserver(self, selector: #selector(sceneDidActivate(_:)),
                           name: UIScene.didActivateNotification, object: nil)
        for name in [UITextField.textDidBeginEditingNotification, UITextView.textDidBeginEditingNotification] {
            center.addObserver(self, selector: #selector(otherBeganEditing(_:)), name: name, object: nil)
        }
        for name in [UITextField.textDidEndEditingNotification, UITextView.textDidEndEditingNotification] {
            center.addObserver(self, selector: #selector(otherEndedEditing(_:)), name: name, object: nil)
        }
    }

    required init?(coder: NSCoder) { return nil }

    deinit { NotificationCenter.default.removeObserver(self) }

    override func willMove(toWindow newWindow: UIWindow?) {
        pictureTap.view?.removeGestureRecognizer(pictureTap)
        if newWindow == nil {
            // Resign before UIKit detaches the responder, preserving a JS
            // active request across a temporary move between windows.
            changingResponder = true
            textView.releasePressedKeys()
            textView.resignFirstResponder()
            changingResponder = false
            automaticallyCapturingHardware = false
            lastKeyboardFrame = nil
        }
        super.willMove(toWindow: newWindow)
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if let window {
            window.addGestureRecognizer(pictureTap)
            hardwareKeyboard.refresh()
            syncResponder()
        }
    }

    @objc(configureActive:inputMode:stickyMods:os:)
    func configure(active: Bool, inputMode: String, stickyMods: Int32, os: String) {
        configured = true
        let nextMode = KeyboardInputMode(rawValue: inputMode) ?? .text
        let nextOS = KeyboardOS(rawValue: os) ?? .macos
        let nextSticky = UInt8(truncatingIfNeeded: stickyMods)
        if nextOS != self.os { textView.releasePressedKeys() }
        self.os = nextOS
        if nextSticky != suppliedStickyMods {
            suppliedStickyMods = nextSticky
            self.stickyMods = nextSticky
        }
        if active != suppliedActive {
            suppliedActive = active
            wantsFirstResponder = active
        }
        if nextMode != mode {
            mode = nextMode
            applyKeyboardTraits()
            // UIKit sometimes retains the previous keyboard layout after a
            // reload alone. Suppress active-change events during this bounce.
            changingResponder = true
            textView.reloadInputViews()
            if textView.isFirstResponder {
                textView.resignFirstResponder()
            }
            changingResponder = false
        }
        syncResponder()
    }

    @objc func reset() {
        configured = false
        syncPaste = false
        automaticallyCapturingHardware = false
        suppliedActive = false
        wantsFirstResponder = false
        changingResponder = true
        textView.releasePressedKeys()
        textView.resignFirstResponder()
        changingResponder = false
        suppliedStickyMods = 0
        stickyMods = 0
        clearText()
        composition = ""
        keyboardVisible = false
        lastKeyboardFrame = nil
    }

    func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
        // Return while marked text exists belongs to the IME's candidate
        // selection. A committed newline is also handled by the planner.
        if text == "\n", textView.markedTextRange == nil {
            dispatchCommit("\n")
            return false
        }
        return true
    }

    func textViewDidChange(_ textView: UITextView) {
        guard !clearingText else { return }
        if let marked = textView.markedTextRange {
            composition = textView.text(in: marked) ?? ""
            onComposition?(["text": composition])
            return
        }
        let committed = textView.text ?? ""
        guard !committed.isEmpty || !composition.isEmpty else { return }
        dispatchCommit(committed)
        clearText()
        composition = ""
        onComposition?(["text": ""])
    }

    private func dispatchCommit(_ committed: String) {
        for action in KeyboardCommitPlanner.plan(committed: committed, mode: mode, stickyMods: stickyMods) {
            switch action {
            case let .text(text):
                onText?(["text": text])
            case let .key(usage, down, mods):
                sendKey(usage: usage, down: down, mods: mods)
            case let .stickyConsumed(mods):
                // Do not re-use a consumed mask while JS is still updating
                // the toolbar prop. Only a new prop value can arm it again.
                stickyMods = 0
                onStickyConsumed?(["mods": Int(mods)])
            }
        }
    }

    func textViewDidBeginEditing(_ textView: UITextView) {
        if !changingResponder, !automaticallyCapturingHardware { onActiveChange?(["active": true]) }
    }

    func textViewDidEndEditing(_ textView: UITextView) {
        self.textView.releasePressedKeys()
        if !composition.isEmpty {
            clearText()
            composition = ""
            onComposition?(["text": ""])
        }
        if !changingResponder, !automaticallyCapturingHardware {
            wantsFirstResponder = false
            onActiveChange?(["active": false])
        }
    }

    private func applyKeyboardTraits() {
        textView.captureMode = mode
        textView.keyboardType = mode == .keys ? .asciiCapable : .default
        textView.autocapitalizationType = .none
        textView.autocorrectionType = .no
        textView.spellCheckingType = mode == .keys ? .no : .default
        textView.smartQuotesType = mode == .keys ? .no : .default
        textView.smartDashesType = mode == .keys ? .no : .default
        textView.smartInsertDeleteType = mode == .keys ? .no : .default
    }

    private func syncResponder() {
        let shouldCapture = KeyboardFocusPolicy.shouldCapture(
            configured: configured, appActive: appActive, inWindow: window != nil,
            wanted: wantsFirstResponder, hardware: hardwareKeyboard.isConnected,
            othersEditing: !othersEditing.isEmpty)
        automaticallyCapturingHardware = shouldCapture && !wantsFirstResponder
        changingResponder = true
        if shouldCapture {
            if !textView.isFirstResponder { textView.becomeFirstResponder() }
        } else if textView.isFirstResponder {
            textView.releasePressedKeys()
            textView.resignFirstResponder()
        }
        changingResponder = false
        if automaticallyCapturingHardware, textView.isFirstResponder {
            publishFrame(height: 0)
        }
    }

    @objc private func willResignActive() {
        appActive = false
        textView.stopRepeat(.background)
        syncResponder()
    }

    @objc private func otherBeganEditing(_ notification: Notification) {
        guard let other = notification.object as? UIView, other !== textView else { return }
        othersEditing.insert(ObjectIdentifier(other))
    }

    @objc private func otherEndedEditing(_ notification: Notification) {
        guard let other = notification.object as? UIView, other !== textView else { return }
        othersEditing.remove(ObjectIdentifier(other))
        guard othersEditing.isEmpty else { return }
        // Focus may be moving on to the next field; look once UIKit settles,
        // then take a hardware keyboard back (a closed credential sheet).
        DispatchQueue.main.async { [weak self] in
            guard let self, self.othersEditing.isEmpty else { return }
            self.syncResponder()
        }
    }

    /// A tap on the remote picture re-checks the focus, e.g. after an alert
    /// took it without a text field to hand it back.
    @objc private func pictureTapped(_ gesture: UITapGestureRecognizer) {
        guard gesture.state == .ended, !textView.isFirstResponder else { return }
        hardwareKeyboard.refresh()
        syncResponder()
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        var view = touch.view
        while let current = view {
            if current is RemoteSessionSurface { return true }
            view = current.superview
        }
        return false
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }

    @objc private func didBecomeActive() {
        // The app is active when any of its windows is; this one may not be.
        if let scene = window?.windowScene, scene.activationState != .foregroundActive { return }
        appActive = true
        hardwareKeyboard.refresh()
        syncResponder()
    }

    @objc private func sceneWillDeactivate(_ notification: Notification) {
        guard let scene = notification.object as? UIScene, scene === window?.windowScene else { return }
        willResignActive()
    }

    @objc private func sceneDidActivate(_ notification: Notification) {
        guard let scene = notification.object as? UIScene, scene === window?.windowScene else { return }
        appActive = true
        hardwareKeyboard.refresh()
        syncResponder()
    }

    private func clearText() {
        clearingText = true
        textView.text = ""
        clearingText = false
    }

    private func connectShortcuts() {
        guard onShortcut != nil else {
            textView.onShortcut = nil
            return
        }
        textView.onShortcut = { [weak self] shortcut in
            self?.onShortcut?(["action": shortcut.action, "index": shortcut.index])
        }
    }

    private func sendKey(usage: UInt16, down: Bool, mods: UInt8) {
        let mapped = KeyboardOsMap.remap(usage: usage, mods: mods, os: os)
        onKey?(["usage": Int(mapped.0), "down": down, "mods": Int(mapped.1)])
    }

    @objc private func keyboardFrameChanged(_ notification: Notification) {
        guard let window,
              textView.isFirstResponder || wantsFirstResponder || keyboardVisible,
              let frame = notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue else { return }
        let keyboardFrame = window.convert(frame.cgRectValue, from: window.screen.coordinateSpace)
        let overlap = window.bounds.intersection(keyboardFrame)
        let touchesBottom = keyboardFrame.maxY >= window.bounds.maxY - 1
        let height = !overlap.isNull && touchesBottom ? max(0, overlap.height) : 0
        publishFrame(height: height)
    }

    @objc private func keyboardWillHide(_ notification: Notification) {
        guard window != nil, textView.isFirstResponder || keyboardVisible || changingResponder else { return }
        publishFrame(height: 0)
        // iPad's dismiss key can hide the software keyboard while retaining
        // the responder. Hardware-keyboard attachment must keep it available.
        if !changingResponder, textView.isFirstResponder, !hardwareKeyboard.isConnected,
           UIApplication.shared.applicationState == .active {
            wantsFirstResponder = false
            changingResponder = true
            textView.resignFirstResponder()
            changingResponder = false
            onActiveChange?(["active": false])
        }
    }

    private func publishFrame(height: CGFloat) {
        let height = automaticallyCapturingHardware ? 0 : height
        let frame = KeyboardFrame(height: height, visible: height > 0,
                                  hardware: hardwareKeyboard.isConnected)
        keyboardVisible = frame.visible
        guard lastKeyboardFrame != frame else { return }
        lastKeyboardFrame = frame
        onKeyboardFrame?([
            "height": Double(frame.height),
            "visible": frame.visible,
            "hardware": frame.hardware,
        ])
    }
}
