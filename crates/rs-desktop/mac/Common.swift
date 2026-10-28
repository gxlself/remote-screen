import Foundation
import Darwin

/// Rust owns the output buffer. Swift allocates it and Rust releases it with
/// `rs_dt_buf_free` after copying the bytes.
@frozen public struct RsDtBuf {
    public var ptr: UnsafeMutablePointer<UInt8>?
    public var len: Int

    public init(ptr: UnsafeMutablePointer<UInt8>? = nil, len: Int = 0) {
        self.ptr = ptr
        self.len = len
    }
}

@_cdecl("rs_dt_buf_free")
public func rs_dt_buf_free(_ ptr: UnsafeMutablePointer<UInt8>?) {
    guard let ptr else { return }
    free(UnsafeMutableRawPointer(ptr))
}

func makeBuf(_ data: Data) -> RsDtBuf {
    guard !data.isEmpty else { return RsDtBuf() }
    guard let ptr = malloc(data.count)?.assumingMemoryBound(to: UInt8.self) else {
        return RsDtBuf()
    }
    data.withUnsafeBytes { bytes in
        guard let base = bytes.baseAddress else { return }
        memcpy(ptr, base, data.count)
    }
    return RsDtBuf(ptr: ptr, len: data.count)
}

/// Store a Swift-owned buffer into the Rust `RsDtBuf` memory supplied through
/// an opaque C pointer. The helper itself is Swift-only, so the Swift struct
/// never appears in an `@_cdecl` signature.
func storeBuf(_ output: UnsafeMutableRawPointer?, _ data: Data) {
    guard let output else { return }
    output.assumingMemoryBound(to: RsDtBuf.self).pointee = makeBuf(data)
}

/// Write an UTF-8 error into a caller-owned buffer, truncating it and keeping
/// one byte for the terminating NUL.
func writeError(
    _ message: String,
    _ output: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) {
    guard let output, capacity > 0 else { return }
    let bytes = Array(message.utf8)
    let count = min(bytes.count, max(0, capacity - 1))
    if count > 0 {
        for index in 0..<count {
            output[index] = CChar(bitPattern: bytes[index])
        }
    }
    output[count] = 0
}

/// Run synchronously on the main thread, or directly when already there.
func onMainSync<T>(_ f: () -> T) -> T {
    if Thread.isMainThread {
        return f()
    }
    return DispatchQueue.main.sync(execute: f)
}

/// Schedule work asynchronously on the main thread.
func onMainAsync(_ f: @escaping () -> Void) {
    DispatchQueue.main.async(execute: f)
}
