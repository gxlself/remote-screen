import Foundation

enum SecureKeyStats {
    static func summary(_ samplesMs: [Double]) -> (p50: Double, p95: Double, max: Double) {
        guard !samplesMs.isEmpty else { return (0, 0, 0) }
        let sorted = samplesMs.sorted()

        func nearestRank(_ percentile: Double) -> Double {
            let rank = max(1, Int(ceil(percentile * Double(sorted.count))))
            return sorted[rank - 1]
        }

        return (nearestRank(0.50), nearestRank(0.95), sorted[sorted.count - 1])
    }
}
