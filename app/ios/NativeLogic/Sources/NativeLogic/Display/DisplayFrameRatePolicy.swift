/// A display preference only. Video decoding and sample-buffer presentation
/// continue independently of the display link that applies this range.
public enum DisplayFrameRatePolicy {
    public struct Range: Equatable, Sendable {
        public let minimum: Int
        public let maximum: Int
        public let preferred: Int

        public init(minimum: Int, maximum: Int, preferred: Int) {
            self.minimum = minimum
            self.maximum = maximum
            self.preferred = preferred
        }
    }

    public static func range(maxFps: Int, lowPower: Bool, streaming: Bool) -> Range? {
        guard streaming, maxFps > 60 else { return nil }
        if lowPower { return Range(minimum: 30, maximum: 60, preferred: 60) }
        let maximum = min(maxFps, 120)
        // 80 Hz is the P11 default lower bound for a 120 Hz display.
        return Range(minimum: min(80, maximum), maximum: maximum, preferred: maximum)
    }
}
