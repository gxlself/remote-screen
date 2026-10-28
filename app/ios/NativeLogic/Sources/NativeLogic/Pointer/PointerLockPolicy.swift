import CoreGraphics
import Foundation

public enum PointerRoute: String, Equatable {
    case relative
    case absolute
    case idle
}

public enum PointerLockReason: String, Equatable {
    case noMouse
    case notStreaming
    case hidden
    case escaped
    case locked
    case notFullScreen
    case pending
}

public struct PointerLockInputs: Equatable {
    public var mouseConnected: Bool
    public var streaming: Bool
    public var visible: Bool
    public var fullScreen: Bool
    public var escaped: Bool
    public var systemLocked: Bool

    public init(mouseConnected: Bool = false,
                streaming: Bool = false,
                visible: Bool = false,
                fullScreen: Bool = false,
                escaped: Bool = false,
                systemLocked: Bool = false) {
        self.mouseConnected = mouseConnected
        self.streaming = streaming
        self.visible = visible
        self.fullScreen = fullScreen
        self.escaped = escaped
        self.systemLocked = systemLocked
    }
}

public enum PointerLockPolicy {
    public static func wantsLock(_ inputs: PointerLockInputs) -> Bool {
        inputs.mouseConnected && inputs.streaming && inputs.visible && !inputs.escaped
    }

    public static func reason(_ inputs: PointerLockInputs) -> PointerLockReason {
        if !inputs.mouseConnected { return .noMouse }
        if !inputs.streaming { return .notStreaming }
        if !inputs.visible { return .hidden }
        if inputs.escaped { return .escaped }
        if inputs.systemLocked { return .locked }
        if !inputs.fullScreen { return .notFullScreen }
        return .pending
    }

    public static func route(_ inputs: PointerLockInputs) -> PointerRoute {
        if inputs.systemLocked { return .relative }
        if inputs.escaped { return .idle }
        return .absolute
    }

    public static func isFullScreen(window: CGSize, screen: CGSize) -> Bool {
        guard window.width.isFinite, window.height.isFinite,
              screen.width.isFinite, screen.height.isFinite,
              window.width > 0, window.height > 0,
              screen.width > 0, screen.height > 0 else {
            return false
        }

        let sameOrientation = abs(window.width - screen.width) <= 1
            && abs(window.height - screen.height) <= 1
        let rotatedOrientation = abs(window.width - screen.height) <= 1
            && abs(window.height - screen.width) <= 1
        return sameOrientation || rotatedOrientation
    }
}
