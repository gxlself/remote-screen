import CoreGraphics
import Foundation

public struct TrackpadClickCounter {
    private var last: (position: CGPoint, time: TimeInterval, count: Int)?

    public init() {}

    public mutating func register(position: CGPoint, time: TimeInterval) -> Int {
        var count = 1
        if let last, time >= last.time, time - last.time < 0.4,
           hypot(position.x - last.position.x, position.y - last.position.y) < 0.01 {
            count = min(last.count + 1, 3)
        }
        last = (position, time, count)
        return count
    }
}
