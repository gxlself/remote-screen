import CoreGraphics
import Foundation

public enum SessionIdParse {
    public static func parse(_ value: String) -> UInt64? {
        guard !value.isEmpty, value.utf8.count <= 20,
              value.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
        var result: UInt64 = 0
        for byte in value.utf8 {
            let digit = UInt64(byte - 48)
            if result > (UInt64.max - digit) / 10 { return nil }
            result = result * 10 + digit
        }
        return result
    }
}

public enum SessionCodecDecision: Equatable {
    case hevc
    case h264
    case noParameterSets
}

public enum SessionCodec {
    public static func detect(vpsLen: Int, spsLen: Int, ppsLen: Int) -> SessionCodecDecision {
        if vpsLen > 0 && spsLen > 0 && ppsLen > 0 { return .hevc }
        if vpsLen == 0 && spsLen > 0 && ppsLen > 0 { return .h264 }
        return .noParameterSets
    }
}

public enum SessionEpochChange: Equatable {
    case first
    case same
    case changed(from: UInt32, to: UInt32)
}

public struct SessionEpochTracker {
    private var current: UInt32?

    public init() {}

    public mutating func observe(_ epoch: UInt32) -> SessionEpochChange {
        guard let previous = current else {
            current = epoch
            return .first
        }
        guard previous != epoch else { return .same }
        current = epoch
        return .changed(from: previous, to: epoch)
    }
}

public enum SessionContentRect {
    public static func fit(video: CGSize, in bounds: CGSize) -> CGRect {
        guard video.width > 0, video.height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let scale = min(bounds.width / video.width, bounds.height / video.height)
        let size = CGSize(width: video.width * scale, height: video.height * scale)
        return CGRect(x: (bounds.width - size.width) / 2,
                      y: (bounds.height - size.height) / 2,
                      width: size.width,
                      height: size.height)
    }
}
