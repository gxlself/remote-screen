import CoreGraphics
import CoreVideo
import UIKit
import VideoToolbox

/// The last picture of each open session, shown beside the current one while
/// three fingers swipe between computers and under a session's page until its
/// first frame arrives. At most six, least recently used goes
/// first, scaled to the screen and no wider than 1920 pixels.
final class SessionSnapshotStore {
    static let shared = SessionSnapshotStore()
    static let didChange = Notification.Name("RsSessionSnapshotDidChange")

    private let lock = NSLock()
    private var lru = SessionSnapshotLRU<String>()
    private var images: [String: CGImage] = [:]
    private let queue = DispatchQueue(label: "rs.session.snapshot", qos: .utility)

    func image(for sessionId: String) -> CGImage? {
        lock.lock()
        defer { lock.unlock() }
        guard let image = images[sessionId] else { return nil }
        lru.touch(sessionId)
        return image
    }

    /// Converts and stores `buffer` off the main thread.
    func store(_ buffer: CVPixelBuffer, for sessionId: String, limit: Int) {
        guard !sessionId.isEmpty else { return }
        queue.async { [weak self] in
            guard let self, let image = Self.scaledImage(buffer, limit: limit) else { return }
            self.put(image, for: sessionId)
        }
    }

    /// Stores an already converted picture (the zoomed-in backdrop).
    func store(_ image: CGImage, for sessionId: String, limit: Int) {
        guard !sessionId.isEmpty else { return }
        queue.async { [weak self] in
            guard let self, let scaled = Self.scaled(image, limit: limit) else { return }
            self.put(scaled, for: sessionId)
        }
    }

    func remove(_ sessionId: String) {
        lock.lock()
        lru.remove(sessionId)
        let removed = images.removeValue(forKey: sessionId) != nil
        lock.unlock()
        if removed { RsLog.marker("SNAPSHOT", ["released": sessionId]) }
    }

    /// Releases every snapshot of a session that is no longer open.
    func retain(only live: [String]) {
        guard !live.isEmpty else { return }
        lock.lock()
        let dropped = lru.retain(only: Set(live))
        for id in dropped { images.removeValue(forKey: id) }
        lock.unlock()
        if !dropped.isEmpty { RsLog.marker("SNAPSHOT", ["released": dropped]) }
    }

    private func put(_ image: CGImage, for sessionId: String) {
        lock.lock()
        images[sessionId] = image
        let evicted = lru.insert(sessionId)
        for id in evicted { images.removeValue(forKey: id) }
        let count = images.count
        lock.unlock()
        RsLog.marker("SNAPSHOT", ["stored": sessionId, "w": image.width, "h": image.height,
                                  "count": count, "evicted": evicted])
        DispatchQueue.main.async {
            NotificationCenter.default.post(name: Self.didChange, object: nil,
                                            userInfo: ["sessionId": sessionId])
        }
    }

    /// The screen's long side in pixels: snapshots never need more.
    static func screenLimit(_ screen: UIScreen?) -> Int {
        let bounds = (screen ?? UIScreen.main).nativeBounds
        return Int(max(bounds.width, bounds.height))
    }

    private static func scaledImage(_ buffer: CVPixelBuffer, limit: Int) -> CGImage? {
        var image: CGImage?
        guard VTCreateCGImageFromCVPixelBuffer(buffer, options: nil, imageOut: &image) == noErr,
              let image else { return nil }
        return scaled(image, limit: limit)
    }

    private static func scaled(_ image: CGImage, limit: Int) -> CGImage? {
        let size = SessionSnapshotLRU<String>.scaledSize(width: image.width, height: image.height, limit: limit)
        guard size.width > 0, size.height > 0 else { return nil }
        if size.width == image.width, size.height == image.height { return image }
        guard let context = CGContext(data: nil, width: size.width, height: size.height, bitsPerComponent: 8,
                                      bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue
                                          | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        context.interpolationQuality = .medium
        context.draw(image, in: CGRect(x: 0, y: 0, width: size.width, height: size.height))
        return context.makeImage()
    }
}
