package ai.gxlself.remotescreen.rsnative.lifecycle

import android.app.Application

object LifecycleBootstrap {
  fun start(app: Application) {
    LifecycleMonitor.start(app)
  }
}
