import Foundation

enum RsLog {
  static func marker(_ tag: String, _ fields: [String: Any]) {
    NSLog("%@", RsMarkerLine.format(tag: tag, fields: fields))
  }
}
