import React
import React_RCTAppDelegate
import UIKit

/// one delegate per scene. Every scene shows a surface of the one
/// React Native host (RsReactRuntime); none starts a JS runtime of its own.
/// A scene opened or restored with a window request is that computer's
/// window (or a 选电脑 window); the first plain scene is the main window, of
/// which there is one.
class SceneDelegate: UIResponder, UIWindowSceneDelegate {
  var window: UIWindow?
  private var windowId = WindowActivity.mainWindowId
  private var request: WindowRequest?

  func scene(
    _ scene: UIScene,
    willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions
  ) {
    guard let windowScene = scene as? UIWindowScene else {
      return
    }
    let coordinator = RsWindowCoordinator.shared
    var request = Self.request(session: session, options: connectionOptions)

    if request == nil, coordinator.isDuplicateMain(session) {
      // iPadOS opened a window by itself (the multitasking menu, App
      // Exposé's "+") while the main one is up: the app keeps one main
      // window, so this one starts on the computer list (选电脑).
      request = WindowActivity.pickerRequest(random: UUID().uuidString)
    }

    let window = UIWindow(windowScene: windowScene)
    self.window = window
    self.request = request
    if let request {
      windowId = request.windowId
      session.userInfo = WindowActivity.userInfo(request)
      windowScene.title = request.title
      coordinator.sceneConnected(windowId, session: session)
      RsReactRuntime.shared.showWindow(request, in: window)
    } else {
      coordinator.sceneConnected(windowId, session: session)
      RsReactRuntime.shared.showMain(in: window, connectionOptions: connectionOptions)
    }
    coordinator.sceneChanged(windowId, phase: Self.phase(windowScene.activationState))
  }

  /// The request from the opening activity, then the restored one, then what
  /// the session kept; nil for the main window.
  private static func request(
    session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions
  ) -> WindowRequest? {
    for activity in connectionOptions.userActivities where activity.activityType == WindowActivity.type {
      if let request = WindowActivity.decode(activity.userInfo) {
        return request
      }
    }
    if let restored = session.stateRestorationActivity,
       restored.activityType == WindowActivity.type,
       let request = WindowActivity.decode(restored.userInfo) {
      return request
    }
    return WindowActivity.decode(sessionInfo: session.userInfo)
  }

  private static func phase(_ state: UIScene.ActivationState) -> WindowPhase {
    switch state {
    case .foregroundActive: return .active
    case .foregroundInactive: return .inactive
    default: return .background
    }
  }

  func sceneDidBecomeActive(_ scene: UIScene) {
    RsWindowCoordinator.shared.sceneChanged(windowId, phase: .active)
  }

  func sceneWillResignActive(_ scene: UIScene) {
    RsWindowCoordinator.shared.sceneChanged(windowId, phase: .inactive)
  }

  func sceneWillEnterForeground(_ scene: UIScene) {
    RsWindowCoordinator.shared.sceneChanged(windowId, phase: .inactive)
  }

  func sceneDidEnterBackground(_ scene: UIScene) {
    RsWindowCoordinator.shared.sceneChanged(windowId, phase: .background)
  }

  /// Not gone yet: iPadOS may connect the session again (its surface waits);
  /// a discarded session closes the window for good (AppDelegate).
  func sceneDidDisconnect(_ scene: UIScene) {
    guard window != nil else { return }
    RsWindowCoordinator.shared.sceneChanged(windowId, phase: .background)
  }

  /// Brings the window back after a relaunch.
  func stateRestorationActivity(for scene: UIScene) -> NSUserActivity? {
    request.map(RsWindowCoordinator.activity(for:))
  }

  func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
    RCTLinkingManager.scene(scene, openURLContexts: URLContexts)
    // Links are handled by the main window's navigator.
    if request != nil {
      RsWindowCoordinator.shared.activate(WindowActivity.mainWindowId)
    }
  }

  func scene(_ scene: UIScene, continue userActivity: NSUserActivity) {
    guard userActivity.activityType != WindowActivity.type else { return }
    RCTLinkingManager.scene(scene, continue: userActivity)
  }
}
