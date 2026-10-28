import Foundation

enum PipRenderMode: Equatable {
    case decode
    case passthrough
}

enum PipRenderStep: Equatable {
    case flushKeepingImage
    case invalidateDecoder
    case requestKeyframe
}

enum PipFrameRoute: Equatable {
    case drop
    case decode
    case passthrough
}

struct PipRenderSwitch {
    private(set) var mode: PipRenderMode = .decode
    private(set) var awaitingKeyframe = false

    mutating func request(_ next: PipRenderMode) -> [PipRenderStep] {
        guard mode != next else { return [] }
        mode = next
        awaitingKeyframe = true
        return [.flushKeepingImage, .invalidateDecoder, .requestKeyframe]
    }

    mutating func route(keyframe: Bool) -> PipFrameRoute {
        if awaitingKeyframe {
            guard keyframe else { return .drop }
            awaitingKeyframe = false
        }
        switch mode {
        case .decode:
            return .decode
        case .passthrough:
            return .passthrough
        }
    }

    mutating func resetForAttach() {
        awaitingKeyframe = false
    }
}
