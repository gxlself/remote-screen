import Foundation

struct SecureKeyRegistrationSnapshot {
    let registered: Bool
    let secureEnclave: Bool
    let fingerprint: String
    let deviceId: String
    let rotatedThisLaunch: Bool
    let error: String?
}

enum SecureKeySigner {
    private static let lock = NSLock()
    private static var retainedSpki: Data?
    private static var registration: SecureKeyRegistrationSnapshot?

    private static let secureKeySignCallback: RsSecureKeySignFn = { msg, len, sigOut, sigLen in
        guard let msg, len > 0, len <= 1_048_576 else { return -1 }
        let message = Data(bytes: msg, count: len)
        do {
            let signature = try SecureKeyStore.shared.sign(message)
            return SecureKeySignBuffer.write(signature, to: sigOut, length: sigLen)
        } catch {
            return -1
        }
    }

    static func registerAtLaunch() {
        do {
            let info = try SecureKeyStore.shared.loadOrCreate()
            let spki = info.spki
            let fp = SecureKeySpki.fingerprint(spki)
            let fingerprint = SecureKeySpki.formatFingerprint(fp)
            let deviceId = SecureKeySpki.deviceId(fp)

            lock.lock()
            retainedSpki = spki
            lock.unlock()
            spki.withUnsafeBytes { bytes in
                let pointer = bytes.baseAddress?.assumingMemoryBound(to: UInt8.self)
                rs_secure_key_register_signer(secureKeySignCallback, pointer, bytes.count)
            }

            let snapshot = SecureKeyRegistrationSnapshot(
                registered: true,
                secureEnclave: info.secureEnclave,
                fingerprint: fingerprint,
                deviceId: deviceId,
                rotatedThisLaunch: info.rotatedThisLaunch,
                error: nil
            )
            lock.lock()
            registration = snapshot
            lock.unlock()
            RsLog.marker("SECUREKEY", [
                "registered": true,
                "secureEnclave": info.secureEnclave,
                "rotated": info.rotatedThisLaunch,
            ])
        } catch {
            let description = error.localizedDescription
            let snapshot = SecureKeyRegistrationSnapshot(
                registered: false,
                secureEnclave: false,
                fingerprint: "",
                deviceId: "",
                rotatedThisLaunch: false,
                error: description
            )
            lock.lock()
            registration = snapshot
            lock.unlock()
            RsLog.marker("SECUREKEY", ["registered": false, "error": description])
        }
    }

    static func snapshot() -> SecureKeyRegistrationSnapshot {
        lock.lock()
        let current = registration
        lock.unlock()
        if let current { return current }

        do {
            let info = try SecureKeyStore.shared.loadOrCreate()
            let fp = SecureKeySpki.fingerprint(info.spki)
            return SecureKeyRegistrationSnapshot(
                registered: false,
                secureEnclave: info.secureEnclave,
                fingerprint: SecureKeySpki.formatFingerprint(fp),
                deviceId: SecureKeySpki.deviceId(fp),
                rotatedThisLaunch: info.rotatedThisLaunch,
                error: nil
            )
        } catch {
            return SecureKeyRegistrationSnapshot(
                registered: false,
                secureEnclave: false,
                fingerprint: "",
                deviceId: "",
                rotatedThisLaunch: false,
                error: error.localizedDescription
            )
        }
    }

    static func benchmark(_ count: Int) throws -> (count: Int, p50Ms: Double, p95Ms: Double, maxMs: Double) {
        let boundedCount = min(max(count, 1), 500)
        var samples: [Double] = []
        samples.reserveCapacity(boundedCount)
        for _ in 0..<boundedCount {
            let length = Int.random(in: 32...512)
            let message = Data((0..<length).map { _ in UInt8.random(in: .min ... .max) })
            do {
                let measured = try SecureKeyStore.shared.signWithDuration(message)
                samples.append(measured.durationMs)
            } catch {
                throw NSError(
                    domain: "SecureKey",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: error.localizedDescription]
                )
            }
        }
        let summary = SecureKeyStats.summary(samples)
        return (
            boundedCount,
            roundToHundredth(summary.p50),
            roundToHundredth(summary.p95),
            roundToHundredth(summary.max)
        )
    }

    private static func roundToHundredth(_ value: Double) -> Double {
        (value * 100).rounded() / 100
    }
}

@objc(RsSecureKeyBridge)
final class RsSecureKeyBridge: NSObject {
    @objc static func info() -> NSDictionary {
        let snapshot = SecureKeySigner.snapshot()
        return [
            "registered": snapshot.registered,
            "secureEnclave": snapshot.secureEnclave,
            "fp": snapshot.fingerprint,
            "deviceId": snapshot.deviceId,
            "rotatedThisLaunch": snapshot.rotatedThisLaunch,
        ] as NSDictionary
    }

    @objc static func benchmark(_ count: NSNumber) throws -> NSDictionary {
        let result = try SecureKeySigner.benchmark(count.intValue)
        return [
            "count": result.count,
            "p50Ms": result.p50Ms,
            "p95Ms": result.p95Ms,
            "maxMs": result.maxMs,
        ] as NSDictionary
    }
}
