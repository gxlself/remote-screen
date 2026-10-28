enum RsNativeBootstrap {
  static func start() {
    SecureKeySigner.registerAtLaunch()
    AdapterSecretStore.registerAtLaunch()
    RecordingKeyBridge.registerAtLaunch()
    RsLifecycleMonitor.shared.start()
    CrashNativeCollector.shared.start()
  }
}
