@preconcurrency import AppKit
import Foundation

/// A single remote session window. All methods that touch AppKit state are
/// called on the main queue by the C entry points below.
public final class RsDtSessionWindow: NSWindow, NSWindowDelegate {
    private var callbacks: RsDtWindowCallbacks?
    private var sessionInput: RsDtInputView!
    private var pipeline: RsDtVideoPipeline!
    private var released = false
    private var closeRequested = false

    public init(
        sessionId _: UInt64,
        title: String,
        width: UInt32,
        height: UInt32,
        callbacks: RsDtWindowCallbacks
    ) {
        self.callbacks = callbacks
        let contentWidth = CGFloat(max(1, width))
        let contentHeight = CGFloat(max(1, height))
        let rect = NSRect(x: 0, y: 0, width: contentWidth, height: contentHeight)
        super.init(contentRect: rect, styleMask: [.titled, .closable, .miniaturizable, .resizable],
                   backing: .buffered, defer: false)
        self.title = title
        collectionBehavior = [.fullScreenPrimary]
        isReleasedWhenClosed = false
        delegate = self
        acceptsMouseMovedEvents = true
        sessionInput = RsDtInputView(frame: NSRect(origin: .zero, size: rect.size), callbacks: callbacks)
        contentView = sessionInput
        pipeline = RsDtVideoPipeline(view: sessionInput.videoView)
        center()
        sendResize()
    }

    public var videoPipeline: RsDtVideoPipeline { pipeline }

    private func emit(_ kind: UInt8, _ a: Double = 0, _ b: Double = 0, _ c: Double = 0) {
        guard !released, !closeRequested, let callbacks else { return }
        callbacks.window(callbacks.ctx, kind, a, b, c)
    }

    private func sendResize() {
        guard let input = sessionInput else { return }
        let scale = backingScaleFactor
        emit(0, Double(input.bounds.width), Double(input.bounds.height), scale)
    }

    public func setTitleFromRust(_ title: String) {
        guard !released else { return }
        self.title = title
    }

    public func setFullscreenFromRust(_ fullscreen: Bool) {
        guard !released, fullscreen != styleMask.contains(.fullScreen) else { return }
        toggleFullScreen(nil)
    }

    public func setKeyboardCaptureFromRust(_ captured: Bool) {
        sessionInput.setKeyboardCapture(captured)
    }

    public func setPointerCaptureFromRust(_ capture: UInt8) {
        sessionInput.setPointerCapture(capture)
    }

    public func setCursorImageFromRust(
        hasImage: Bool,
        png: UnsafePointer<UInt8>?,
        length: Int,
        width: UInt32,
        height: UInt32,
        hotX: UInt32,
        hotY: UInt32,
        scale: Float
    ) {
        sessionInput.setCursorImage(hasImage: hasImage, png: png, length: length, width: width,
                                    height: height, hotX: hotX, hotY: hotY, scale: scale)
    }

    public func setCursorHiddenFromRust(_ hidden: Bool) {
        sessionInput.setCursorHidden(hidden)
    }

    public func showRemoteCursorFromRust(_ shown: Bool) {
        sessionInput.videoView.showRemoteCursor(shown)
    }

    public func moveRemoteCursorFromRust(_ x: Float, _ y: Float) {
        sessionInput.videoView.moveRemoteCursor(nx: Double(x), ny: Double(y))
    }

    public func setOverlayFromRust(_ text: String?) {
        sessionInput.videoView.setOverlayText(text)
    }

    public func focusFromRust() {
        guard !released else { return }
        makeKeyAndOrderFront(nil)
        NSApp?.activate(ignoringOtherApps: true)
        makeFirstResponder(sessionInput)
    }

    public func closeFromRust() {
        finishClose()
    }

    /// Unbind the callback context and order the window out. The retained
    /// window object is released by `rs_dt_window_release` after this method.
    public func finishClose() {
        guard !released else { return }
        closeRequested = true
        orderOut(nil)
        sessionInput.detachCallbacks()
        pipeline.close()
        let callbacks = self.callbacks
        if let callbacks {
            callbacks.release(callbacks.ctx)
        }
        self.callbacks = nil
        released = true
        delegate = nil
    }

    public func windowShouldClose(_ sender: NSWindow) -> Bool {
        _ = sender
        guard !released, !closeRequested else { return false }
        emit(4)
        return false
    }

    public func windowDidBecomeKey(_ notification: Notification) {
        _ = notification
        emit(1, 1)
    }

    public func windowDidResignKey(_ notification: Notification) {
        _ = notification
        sessionInput.setPointerCapture(0)
        emit(1, 0)
    }

    public func windowDidEnterFullScreen(_ notification: Notification) {
        _ = notification
        emit(2, 1)
    }

    public func windowDidExitFullScreen(_ notification: Notification) {
        _ = notification
        emit(2, 0)
    }

    public func windowDidMiniaturize(_ notification: Notification) {
        _ = notification
        emit(3, 1)
    }

    public func windowDidDeminiaturize(_ notification: Notification) {
        _ = notification
        emit(3, 0)
    }

    public func windowDidResize(_ notification: Notification) {
        _ = notification
        sendResize()
    }

    public func windowDidChangeBackingProperties(_ notification: Notification) {
        _ = notification
        sendResize()
    }
}

private func windowFromRaw(_ pointer: UnsafeMutableRawPointer?) -> RsDtSessionWindow? {
    guard let pointer else { return nil }
    return Unmanaged<RsDtSessionWindow>.fromOpaque(pointer).takeUnretainedValue()
}

private func copiedCString(_ pointer: UnsafePointer<CChar>?) -> String {
    guard let pointer else { return "" }
    return String(cString: pointer)
}

@_cdecl("rs_dt_window_create")
public func rs_dt_window_create(
    _ sessionId: UInt64,
    _ title: UnsafePointer<CChar>?,
    _ width: UInt32,
    _ height: UInt32,
    _ callbacksPointer: UnsafeRawPointer?
) -> UnsafeMutableRawPointer? {
    guard let callbacksPointer else { return nil }
    // The Rust callback table is a repr(C) value passed by pointer. Copy it
    // before entering the main queue so the caller's stack value need only
    // remain alive for this synchronous FFI call.
    let callbacks = callbacksPointer.assumingMemoryBound(to: RsDtWindowCallbacks.self).pointee
    return onMainSync { () -> UnsafeMutableRawPointer? in
        guard NSApp != nil else { return nil }
        let window = RsDtSessionWindow(sessionId: sessionId, title: copiedCString(title), width: width,
                                       height: height, callbacks: callbacks)
        window.makeKeyAndOrderFront(nil)
        window.focusFromRust()
        NSApp?.activate(ignoringOtherApps: true)
        return Unmanaged.passRetained(window).toOpaque()
    }
}

@_cdecl("rs_dt_window_video")
public func rs_dt_window_video(_ pointer: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? {
    onMainSync {
        guard let window = windowFromRaw(pointer) else { return nil }
        return Unmanaged.passRetained(window.videoPipeline).toOpaque()
    }
}

@_cdecl("rs_dt_window_set_title")
public func rs_dt_window_set_title(_ pointer: UnsafeMutableRawPointer?, _ title: UnsafePointer<CChar>?) {
    let copied = copiedCString(title)
    onMainAsync {
        windowFromRaw(pointer)?.setTitleFromRust(copied)
    }
}

@_cdecl("rs_dt_window_set_fullscreen")
public func rs_dt_window_set_fullscreen(_ pointer: UnsafeMutableRawPointer?, _ fullscreen: Bool) {
    onMainAsync {
        windowFromRaw(pointer)?.setFullscreenFromRust(fullscreen)
    }
}

@_cdecl("rs_dt_window_set_keyboard_capture")
public func rs_dt_window_set_keyboard_capture(_ pointer: UnsafeMutableRawPointer?, _ captured: Bool) {
    onMainAsync {
        windowFromRaw(pointer)?.setKeyboardCaptureFromRust(captured)
    }
}

@_cdecl("rs_dt_window_set_pointer_capture")
public func rs_dt_window_set_pointer_capture(_ pointer: UnsafeMutableRawPointer?, _ capture: UInt8) {
    onMainAsync {
        windowFromRaw(pointer)?.setPointerCaptureFromRust(capture)
    }
}

@_cdecl("rs_dt_window_set_cursor_image")
public func rs_dt_window_set_cursor_image(
    _ pointer: UnsafeMutableRawPointer?,
    _ hasImage: Bool,
    _ png: UnsafePointer<UInt8>?,
    _ length: Int,
    _ width: UInt32,
    _ height: UInt32,
    _ hotX: UInt32,
    _ hotY: UInt32,
    _ scale: Float
) {
    let copied: Data? = if hasImage, let png, length > 0 { Data(bytes: png, count: length) } else { nil }
    onMainAsync {
        copied?.withUnsafeBytes { bytes in
            windowFromRaw(pointer)?.setCursorImageFromRust(
                hasImage: hasImage, png: bytes.bindMemory(to: UInt8.self).baseAddress,
                length: copied?.count ?? 0, width: width, height: height, hotX: hotX, hotY: hotY,
                scale: scale)
        }
        if copied == nil {
            windowFromRaw(pointer)?.setCursorImageFromRust(
                hasImage: false, png: nil, length: 0, width: width, height: height, hotX: hotX,
                hotY: hotY, scale: scale)
        }
    }
}

@_cdecl("rs_dt_window_set_cursor_hidden")
public func rs_dt_window_set_cursor_hidden(_ pointer: UnsafeMutableRawPointer?, _ hidden: Bool) {
    onMainAsync {
        windowFromRaw(pointer)?.setCursorHiddenFromRust(hidden)
    }
}

@_cdecl("rs_dt_window_show_remote_cursor")
public func rs_dt_window_show_remote_cursor(_ pointer: UnsafeMutableRawPointer?, _ shown: Bool) {
    onMainAsync {
        windowFromRaw(pointer)?.showRemoteCursorFromRust(shown)
    }
}

@_cdecl("rs_dt_window_move_remote_cursor")
public func rs_dt_window_move_remote_cursor(_ pointer: UnsafeMutableRawPointer?, _ x: Float, _ y: Float) {
    onMainAsync {
        windowFromRaw(pointer)?.moveRemoteCursorFromRust(x, y)
    }
}

@_cdecl("rs_dt_window_set_overlay")
public func rs_dt_window_set_overlay(_ pointer: UnsafeMutableRawPointer?, _ text: UnsafePointer<CChar>?) {
    let copied = text.map(copiedCString)
    onMainAsync {
        windowFromRaw(pointer)?.setOverlayFromRust(copied)
    }
}

@_cdecl("rs_dt_window_focus")
public func rs_dt_window_focus(_ pointer: UnsafeMutableRawPointer?) {
    onMainAsync {
        windowFromRaw(pointer)?.focusFromRust()
    }
}

@_cdecl("rs_dt_window_close")
public func rs_dt_window_close(_ pointer: UnsafeMutableRawPointer?) {
    onMainAsync {
        windowFromRaw(pointer)?.closeFromRust()
    }
}

@_cdecl("rs_dt_window_release")
public func rs_dt_window_release(_ pointer: UnsafeMutableRawPointer?) {
    guard let pointer else { return }
    onMainAsync {
        let window = Unmanaged<RsDtSessionWindow>.fromOpaque(pointer).takeRetainedValue()
        window.finishClose()
    }
}

@_cdecl("rs_dt_main_screen_pixel_width")
public func rs_dt_main_screen_pixel_width() -> UInt32 {
    onMainSync {
        guard let screen = NSScreen.main else { return 0 }
        let pixels = screen.frame.width * screen.backingScaleFactor
        return UInt32(clamping: Int(pixels.rounded()))
    }
}
