import Foundation

enum SecureKeySignBuffer {
    static func write(
        _ sig: Data,
        to out: UnsafeMutablePointer<UInt8>?,
        length: UnsafeMutablePointer<Int>?
    ) -> Int32 {
        guard let out, let length else { return -3 }
        let capacity = length.pointee
        guard capacity >= sig.count else {
            length.pointee = sig.count
            return -2
        }
        sig.copyBytes(to: out, count: sig.count)
        length.pointee = sig.count
        return 0
    }
}
