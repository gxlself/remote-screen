import UIKit

@main
class AppDelegate: UIResponder, UIApplicationDelegate {
  func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    RsNativeBootstrap.start()
    return true
  }

  /// windows closed in App Exposé (or by JS) are gone for good.
  func application(
    _ application: UIApplication,
    didDiscardSceneSessions sceneSessions: Set<UISceneSession>
  ) {
    RsWindowCoordinator.shared.scenesDiscarded(sceneSessions)
  }
}
