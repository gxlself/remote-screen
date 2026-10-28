import React
import React_RCTAppDelegate
import ReactAppDependencyProvider
import UIKit

/// one React Native host (one JS runtime, one rscore) for every
/// window. Each scene shows a surface of the same `RCTReactNativeFactory`:
/// `rootViewFactory.reactHost` is created by the first surface and reused by
/// `rootViewFactory.view(withModuleName:initialProperties:)` for the others.
///
/// The main surface (`RemoteScreen`) lives as long as the app, even while no
/// scene shows it: a window of its own needs the main tree's services, and a
/// main scene that comes back gets the same surface again. A window's
/// surface (`RemoteScreenWindow`) lives until its scene session is discarded.
final class RsReactRuntime: RCTDefaultReactNativeFactoryDelegate {
  static let shared = RsReactRuntime()

  static let mainModule = "RemoteScreen"
  static let windowModule = "RemoteScreenWindow"

  private var factory: RCTReactNativeFactory?
  /// A controller is the root of one window at a time; each surface keeps its own.
  private var mainController: UIViewController?
  private var windowControllers: [String: UIViewController] = [:]

  private override init() {
    super.init()
  }

  private func reactFactory() -> RCTReactNativeFactory {
    if let factory { return factory }
    dependencyProvider = RCTAppDependencyProvider()
    let created = RCTReactNativeFactory(delegate: self)
    #if DEBUG
    created.devMenuConfiguration = RCTDevMenuConfiguration(
      devMenuEnabled: true,
      shakeGestureEnabled: true,
      keyboardShortcutsEnabled: true
    )
    #endif
    factory = created
    return created
  }

  /// The main window: the whole app, started on first use with the scene's
  /// URL and activity as launch options.
  func showMain(in window: UIWindow, connectionOptions: UIScene.ConnectionOptions) {
    if let controller = mainController {
      show(controller, in: window)
      return
    }
    reactFactory().startReactNative(
      withModuleName: Self.mainModule,
      in: window,
      initialProperties: mainProperties(),
      connectionOptions: connectionOptions
    )
    mainController = window.rootViewController
    RsLog.marker("WINDOW", ["surface": "main"])
  }

  /// A computer's window. iPadOS may bring such a window back before the main
  /// one (after a relaunch); the main surface then starts without a window.
  func showWindow(_ request: WindowRequest, in window: UIWindow) {
    startMainIfNeeded()
    if let controller = windowControllers[request.windowId] {
      show(controller, in: window)
      return
    }
    let view = reactFactory().rootViewFactory.view(
      withModuleName: Self.windowModule,
      initialProperties: [
        "windowId": request.windowId,
        "payload": request.payload,
      ]
    )
    let controller = createRootViewController()
    controller.view = view
    windowControllers[request.windowId] = controller
    RsLog.marker("WINDOW", ["surface": "window"])
    show(controller, in: window)
  }

  /// The scene session is gone: its surface stops and its React root unmounts.
  func dropWindow(_ windowId: String) {
    guard let controller = windowControllers.removeValue(forKey: windowId) else { return }
    controller.view.window?.rootViewController = nil
  }

  private func startMainIfNeeded() {
    guard mainController == nil else { return }
    let view = reactFactory().rootViewFactory.view(
      withModuleName: Self.mainModule,
      initialProperties: mainProperties()
    )
    let controller = createRootViewController()
    controller.view = view
    mainController = controller
    RsLog.marker("WINDOW", ["surface": "main", "headless": true])
  }

  private func show(_ controller: UIViewController, in window: UIWindow) {
    if let previous = controller.view.window, previous !== window {
      previous.rootViewController = nil
    }
    window.rootViewController = controller
    window.makeKeyAndVisible()
  }

  private func mainProperties() -> [String: Any] {
    var initialProperties: [String: Any] = LaunchArguments.properties(ProcessInfo.processInfo.arguments)

    let fileManager = FileManager.default
    if let applicationSupport = fileManager.urls(
      for: .applicationSupportDirectory,
      in: .userDomainMask
    ).first {
      var dataDirectory = LaunchDataDir.url(appSupport: applicationSupport)
      do {
        try fileManager.createDirectory(
          at: dataDirectory,
          withIntermediateDirectories: true,
          attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]
        )
        var resourceValues = URLResourceValues()
        resourceValues.isExcludedFromBackup = true
        try dataDirectory.setResourceValues(resourceValues)
        initialProperties["dataDir"] = dataDirectory.path
      } catch {
        RsLog.marker("BOOT", [
          "dataDir": "error",
          "message": error.localizedDescription,
        ])
      }
    } else {
      RsLog.marker("BOOT", [
        "dataDir": "error",
        "message": "Application Support directory unavailable",
      ])
    }

    let launchKeys = initialProperties.keys.sorted().map { $0 as Any }
    RsLog.marker("LAUNCH", ["keys": launchKeys])
    return initialProperties
  }

  override func createRootViewController() -> UIViewController {
    RsRootViewController()
  }

  override func bundleURL() -> URL? {
    #if DEBUG
    RCTBundleURLProvider.sharedSettings().jsBundleURL(forBundleRoot: "index")
    #else
    Bundle.main.url(forResource: "main", withExtension: "jsbundle")
    #endif
  }
}
