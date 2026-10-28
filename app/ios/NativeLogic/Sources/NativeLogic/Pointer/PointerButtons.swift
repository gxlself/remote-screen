import CoreGraphics
import Foundation

public enum PointerButtons {
    public static let left: Int32 = 0
    public static let right: Int32 = 1
    public static let middle: Int32 = 2
    public static let back: Int32 = 3
    public static let forward: Int32 = 4

    public static func auxiliary(index: Int) -> Int32? {
        switch index {
        case 0: return back
        case 1: return forward
        default: return nil
        }
    }

    public static func fromButtonMask(_ rawValue: Int) -> Int32? {
        guard rawValue > 0 else { return nil }
        let bit = rawValue.trailingZeroBitCount
        guard bit < 5 else { return nil }
        return Int32(bit)
    }
}
