@preconcurrency import AppKit
import CoreGraphics
import Foundation

public typealias RsDtWindowKeyCallback = @convention(c) (
    UnsafeMutableRawPointer?, UInt8, UInt16, Bool, UInt64
) -> Void
public typealias RsDtWindowPointerCallback = @convention(c) (
    UnsafeMutableRawPointer?, UInt8, Double, Double, Double, Double, UInt8, Bool, UInt8
) -> Void
public typealias RsDtWindowScrollCallback = @convention(c) (
    UnsafeMutableRawPointer?, UInt8, Double, Double, UInt64, UInt64
) -> Void
public typealias RsDtWindowCallback = @convention(c) (
    UnsafeMutableRawPointer?, UInt8, Double, Double, Double
) -> Void
public typealias RsDtWindowReleaseCallback = @convention(c) (
    UnsafeMutableRawPointer?
) -> Void

@frozen public struct RsDtWindowCallbacks {
    public var ctx: UnsafeMutableRawPointer?
    public var key: RsDtWindowKeyCallback
    public var pointer: RsDtWindowPointerCallback
    public var scroll: RsDtWindowScrollCallback
    public var window: RsDtWindowCallback
    public var release: RsDtWindowReleaseCallback

    public init(
        ctx: UnsafeMutableRawPointer?,
        key: @escaping RsDtWindowKeyCallback,
        pointer: @escaping RsDtWindowPointerCallback,
        scroll: @escaping RsDtWindowScrollCallback,
        window: @escaping RsDtWindowCallback,
        release: @escaping RsDtWindowReleaseCallback
    ) {
        self.ctx = ctx
        self.key = key
        self.pointer = pointer
        self.scroll = scroll
        self.window = window
        self.release = release
    }
}

/// AppKit input surface used by one session window.
@preconcurrency
public final class RsDtInputView: NSView {
    public let videoView: RsDtVideoView
    private var callbacks: RsDtWindowCallbacks?
    private var tracking: NSTrackingArea?
    private var keyboardCaptured = false
    private var pointerCapture: UInt8 = 0
    private var hostCursor: NSCursor = .arrow
    private var transparentCursor: NSCursor?
    private var cursorHidden = false
    private var pointerCursorHidden = false
    private var cursorDisassociated = false
    private var released = false

    public override var isFlipped: Bool { true }
    public override var acceptsFirstResponder: Bool { true }

    public override func hitTest(_ point: NSPoint) -> NSView? {
        bounds.contains(point) ? self : nil
    }

    public init(frame frameRect: NSRect, callbacks: RsDtWindowCallbacks) {
        videoView = RsDtVideoView(frame: frameRect)
        self.callbacks = callbacks
        super.init(frame: frameRect)
        autoresizingMask = [.width, .height]
        videoView.autoresizingMask = [.width, .height]
        addSubview(videoView)
        window?.acceptsMouseMovedEvents = true
        updateTrackingArea()
    }

    public required init?(coder: NSCoder) {
        return nil
    }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        window?.acceptsMouseMovedEvents = true
        updateTrackingArea()
    }

    public override func layout() {
        super.layout()
        videoView.frame = bounds
        updateTrackingArea()
    }

    private func updateTrackingArea() {
        if let tracking { removeTrackingArea(tracking) }
        let options: NSTrackingArea.Options = [
            .mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect, .cursorUpdate,
        ]
        let area = NSTrackingArea(rect: bounds, options: options, owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    public func detachCallbacks() {
        if pointerCapture == 2 { restoreFreePointer() }
        pointerCapture = 0
        released = true
        callbacks = nil
    }

    public func setKeyboardCapture(_ captured: Bool) {
        guard !released else { return }
        keyboardCaptured = captured
        needsDisplay = true
    }

    public func setPointerCapture(_ capture: UInt8) {
        guard !released else { return }
        let next: UInt8 = capture == 2 ? 2 : 0 // Confine is Free on macOS.
        guard next != pointerCapture else { return }
        if pointerCapture == 2 { restoreFreePointer() }
        pointerCapture = next
        if next == 2 { lockVirtualPointer() }
    }

    private func viewPoint(_ event: NSEvent) -> NSPoint {
        convert(event.locationInWindow, from: nil)
    }

    private func virtualPoint() -> NSPoint {
        let rect = videoView.videoContentRect
        if rect.width > 0, rect.height > 0 { return NSPoint(x: rect.midX, y: rect.midY) }
        return NSPoint(x: bounds.midX, y: bounds.midY)
    }

    private func emitKey(kind: UInt8, event: NSEvent, repeat isRepeat: Bool = false, flags: UInt64 = 0) {
        guard !released, let callbacks else { return }
        callbacks.key(callbacks.ctx, kind, event.keyCode, isRepeat, flags)
    }

    private func emitPointer(
        kind: UInt8,
        point: NSPoint,
        dx: CGFloat = 0,
        dy: CGFloat = 0,
        button: UInt8 = 0,
        down: Bool = false,
        clicks: UInt8 = 0
    ) {
        guard !released, let callbacks else { return }
        callbacks.pointer(callbacks.ctx, kind, Double(point.x), Double(point.y), Double(dx), Double(dy),
                          button, down, clicks)
    }

    private func emitButton(_ event: NSEvent, button: UInt8, down: Bool) {
        let point = pointerPoint(for: event)
        emitPointer(kind: 1, point: point, button: button, down: down,
                    clicks: UInt8(clamping: event.clickCount))
    }

    private func focusInput() {
        window?.makeFirstResponder(self)
    }

    private func pointerPoint(for event: NSEvent) -> NSPoint {
        pointerCapture == 2 ? virtualPoint() : viewPoint(event)
    }

    private func emitMove(_ event: NSEvent) {
        let point = pointerPoint(for: event)
        emitPointer(kind: 0, point: point, dx: event.deltaX, dy: event.deltaY)
    }

    private func emitScroll(_ event: NSEvent) {
        guard !released, let callbacks else { return }
        if event.hasPreciseScrollingDeltas {
            callbacks.scroll(callbacks.ctx, 0, Double(event.scrollingDeltaX), Double(event.scrollingDeltaY),
                             UInt64(event.phase.rawValue), UInt64(event.momentumPhase.rawValue))
        } else {
            callbacks.scroll(callbacks.ctx, 1, Double(event.scrollingDeltaX), Double(event.scrollingDeltaY), 0, 0)
        }
    }

    public override func keyDown(with event: NSEvent) {
        emitKey(kind: 0, event: event, repeat: event.isARepeat)
    }

    public override func keyUp(with event: NSEvent) {
        emitKey(kind: 1, event: event)
    }

    public override func flagsChanged(with event: NSEvent) {
        emitKey(kind: 2, event: event, flags: UInt64(event.modifierFlags.rawValue))
    }

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard keyboardCaptured else { return super.performKeyEquivalent(with: event) }
        emitKey(kind: 0, event: event, repeat: event.isARepeat)
        return true
    }

    public override func mouseMoved(with event: NSEvent) { emitMove(event) }
    public override func mouseDragged(with event: NSEvent) { emitMove(event) }
    public override func rightMouseDragged(with event: NSEvent) { emitMove(event) }
    public override func otherMouseDragged(with event: NSEvent) { emitMove(event) }
    public override func mouseDown(with event: NSEvent) {
        focusInput()
        emitButton(event, button: 0, down: true)
    }
    public override func mouseUp(with event: NSEvent) { emitButton(event, button: 0, down: false) }
    public override func rightMouseDown(with event: NSEvent) {
        focusInput()
        emitButton(event, button: 1, down: true)
    }
    public override func rightMouseUp(with event: NSEvent) { emitButton(event, button: 1, down: false) }

    public override func otherMouseDown(with event: NSEvent) {
        guard let button = protocolButton(event.buttonNumber) else { return }
        focusInput()
        emitButton(event, button: button, down: true)
    }

    public override func otherMouseUp(with event: NSEvent) {
        guard let button = protocolButton(event.buttonNumber) else { return }
        emitButton(event, button: button, down: false)
    }

    private func protocolButton(_ number: Int) -> UInt8? {
        switch number {
        case 2: return 2
        case 3: return 3
        case 4: return 4
        default: return nil
        }
    }

    public override func mouseExited(with event: NSEvent) {
        guard !released else { return }
        emitPointer(kind: 2, point: pointerPoint(for: event))
    }

    public override func scrollWheel(with event: NSEvent) { emitScroll(event) }

    public override func cursorUpdate(with event: NSEvent) {
        ((cursorHidden || pointerCursorHidden) ? transparentCursor : hostCursor)?.set()
    }

    public override func resetCursorRects() {
        super.discardCursorRects()
        let rect = videoView.videoContentRect
        guard rect.width > 0, rect.height > 0 else { return }
        addCursorRect(rect, cursor: (cursorHidden || pointerCursorHidden) ?
                      transparentCursor ?? NSCursor.arrow : hostCursor)
    }

    public func setCursorImage(
        hasImage: Bool,
        png: UnsafePointer<UInt8>?,
        length: Int,
        width: UInt32,
        height: UInt32,
        hotX: UInt32,
        hotY: UInt32,
        scale: Float
    ) {
        let data: Data? = if hasImage, let png, length > 0 {
            Data(bytes: png, count: length)
        } else {
            nil
        }
        if let data, let image = NSImage(data: data), scale.isFinite, scale > 0 {
            let pointScale = CGFloat(scale)
            image.size = NSSize(width: CGFloat(width) / pointScale, height: CGFloat(height) / pointScale)
            hostCursor = NSCursor(
                image: image,
                hotSpot: NSPoint(x: CGFloat(hotX) / pointScale, y: CGFloat(hotY) / pointScale)
            )
            videoView.setCursorImage(png: data, hotX: hotX, hotY: hotY, scale: scale)
        } else {
            hostCursor = .arrow
            videoView.setCursorImage(png: nil, hotX: 0, hotY: 0, scale: 1)
        }
        invalidateCursorRects()
    }

    public func setCursorHidden(_ hidden: Bool) {
        guard !released else { return }
        cursorHidden = hidden
        if hidden {
            if transparentCursor == nil {
                let image = NSImage(size: NSSize(width: 1, height: 1))
                image.lockFocus()
                NSColor.clear.set()
                NSRect(origin: .zero, size: NSSize(width: 1, height: 1)).fill()
                image.unlockFocus()
                transparentCursor = NSCursor(image: image, hotSpot: .zero)
            }
        }
        invalidateCursorRects()
    }

    private func invalidateCursorRects() {
        window?.invalidateCursorRects(for: self)
        resetCursorRects()
    }

    private func lockVirtualPointer() {
        let point = virtualPoint()
        if let window {
            let screenPoint = window.convertToScreen(NSRect(origin: point, size: .zero)).origin
            CGWarpMouseCursorPosition(screenPoint)
        }
        CGAssociateMouseAndMouseCursorPosition(0)
        cursorDisassociated = true
        if !pointerCursorHidden {
            NSCursor.hide()
            pointerCursorHidden = true
        }
    }

    private func restoreFreePointer() {
        if cursorDisassociated {
            CGAssociateMouseAndMouseCursorPosition(1)
            cursorDisassociated = false
        }
        if pointerCursorHidden {
            NSCursor.unhide()
            pointerCursorHidden = false
        }
        invalidateCursorRects()
    }
}
