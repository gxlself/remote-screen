package ai.gxlself.remotescreen.rsnative.jni

interface TermSink {
  // Called on a Rust worker. The array is a callback-local copy and may be kept.
  // Return promptly: never block here or synchronously wait for the main thread.
  fun onOutput(data: ByteArray)

  // Called once when the Rust terminal closes. No output follows this callback.
  fun onClosed()
}
