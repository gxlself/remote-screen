import ObjectiveC
import UIKit

final class RsRootViewController: UIViewController {
    private var loggedAppearance = false

    override var prefersPointerLocked: Bool { PointerLockCoordinator.shared.wantsLock }
    override var childViewControllerForPointerLock: UIViewController? { nil }

    // React Native assigns `view` directly, so `viewDidLoad` never runs here.
    override init(nibName nibNameOrNil: String?, bundle nibBundleOrNil: Bundle?) {
        super.init(nibName: nibNameOrNil, bundle: nibBundleOrNil)
        ScreenEdgeDeferral.install()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        ScreenEdgeDeferral.install()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !loggedAppearance else { return }
        loggedAppearance = true
        RsLog.marker("POINTER", ["rootVC": "RsRootViewController"])
    }

    // react-native-screens reports `autoHideHomeIndicator` changes to the key
    // window's root; the edge deferral follows the same screens.
    override func setNeedsUpdateOfHomeIndicatorAutoHidden() {
        super.setNeedsUpdateOfHomeIndicatorAutoHidden()
        setNeedsUpdateOfScreenEdgesDeferringSystemGestures()
        var top: UIViewController = self
        while let presented = top.presentedViewController { top = presented }
        if top !== self { top.setNeedsUpdateOfScreenEdgesDeferringSystemGestures() }
    }
}

/// react-native-screens has no option for deferring system edge gestures or
/// locking the pointer, and UIKit asks the topmost full-screen modal for both
/// instead of the window's root. The live session screens are such modals.
/// Screens that hide the home indicator (the live session screens) defer the
/// top and bottom edges, so the first swipe only shows the grabber instead of
/// opening Notification / Control Center or leaving the app. The screen that
/// holds the active session surface asks for the pointer lock the coordinator
/// wants; without it the iPad pointer reaches the corners and the Dock. Both
/// preferences go away with the screen when it is dismissed.
enum ScreenEdgeDeferral {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        guard let screenClass = NSClassFromString("RNSScreen") else {
            RsLog.marker("EDGES", ["install": "missing"])
            return
        }
        let edges: @convention(block) (UIViewController) -> UIRectEdge = { controller in
            controller.prefersHomeIndicatorAutoHidden ? [.top, .bottom] : []
        }
        let locked: @convention(block) (UIViewController) -> Bool = { controller in
            let coordinator = PointerLockCoordinator.shared
            guard coordinator.wantsLock, controller.isViewLoaded,
                  let surface = coordinator.activeSurface else { return false }
            return surface.isDescendant(of: controller.view)
        }
        RsLog.marker("EDGES", [
            "install": add(screenClass, #selector(getter: UIViewController.preferredScreenEdgesDeferringSystemGestures),
                           imp_implementationWithBlock(edges)),
            "pointerLock": add(screenClass, #selector(getter: UIViewController.prefersPointerLocked),
                               imp_implementationWithBlock(locked)),
        ])
    }

    /// Adds the getter to RNSScreen only; fails if the library ever defines it.
    private static func add(_ screenClass: AnyClass, _ selector: Selector, _ imp: IMP) -> String {
        guard let base = class_getInstanceMethod(UIViewController.self, selector) else { return "missing" }
        return class_addMethod(screenClass, selector, imp, method_getTypeEncoding(base)) ? "ok" : "exists"
    }
}
