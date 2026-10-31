import Foundation

enum RsLog {
  static func marker(_ tag: String, _ fields: [String: Any]) {
    let line = RsMarkerLine.format(tag: tag, fields: fields)
    NSLog("%@", line)
    RsMarkerFile.shared.append(line)
  }
}

/// iOS 26 hides NSLog text from the device log, and a console attached over
/// the cable ends with the cable. Markers also go to Library/Caches/
/// markers.log (rolled once into markers.1.log) so a run on a device can be
/// read back afterwards.
private final class RsMarkerFile {
  static let shared = RsMarkerFile()
  private static let limit: UInt64 = 2 << 20

  private let queue = DispatchQueue(label: "rs.log.markers", qos: .utility)
  private let url: URL?
  private var handle: FileHandle?
  private var size: UInt64 = 0
  private let clock: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    formatter.timeZone = .current
    return formatter
  }()

  private init() {
    url = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
      .appendingPathComponent("markers.log")
  }

  func append(_ line: String) {
    let now = Date()
    queue.async { [self] in
      guard let data = "\(clock.string(from: now)) \(line)\n".data(using: .utf8),
            let handle = open() else { return }
      do {
        try handle.write(contentsOf: data)
        size += UInt64(data.count)
      } catch {
        close()
      }
    }
  }

  private func open() -> FileHandle? {
    if let handle, size < Self.limit { return handle }
    guard let url else { return nil }
    let files = FileManager.default
    if handle != nil {
      close()
      let rolled = url.deletingLastPathComponent().appendingPathComponent("markers.1.log")
      try? files.removeItem(at: rolled)
      try? files.moveItem(at: url, to: rolled)
    }
    if !files.fileExists(atPath: url.path) {
      files.createFile(atPath: url.path, contents: nil)
    }
    guard let opened = try? FileHandle(forWritingTo: url),
          let end = try? opened.seekToEnd() else { return nil }
    handle = opened
    size = end
    return opened
  }

  private func close() {
    try? handle?.close()
    handle = nil
  }
}
