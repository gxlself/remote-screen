package ai.gxlself.remotescreen.rsnative.securekey

import android.app.Application
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

object SecureKeyBootstrap {
  private val started = AtomicBoolean(false)
  private val worker = Executors.newSingleThreadExecutor { runnable ->
    Thread(runnable, "rs-secure-key-bootstrap").apply { isDaemon = true }
  }

  fun start(app: Application) {
    if (!started.compareAndSet(false, true)) return
    // StrongBox key generation can take hundreds of milliseconds. Registration
    // therefore runs after Application.onCreate on a background thread; Rust
    // signing calls made before it finishes fail until the signer is installed.
    worker.execute { SecureKeySigner.registerAtLaunch(app) }
  }
}
