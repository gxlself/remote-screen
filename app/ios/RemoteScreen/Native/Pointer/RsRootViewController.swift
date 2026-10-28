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
    // window's root; the bottom-edge deferral follows the same screens.
    override func setNeedsUpdateOfHomeIndicatorAutoHidden() {
        super.setNeedsUpdateOfHomeIndicatorAutoHidden()
        setNeedsUpdateOfScreenEdgesDeferringSystemGestures()
        var top: UIViewController = self
        while let presented = top.presentedViewController { top = presented }
        if top !== self { top.setNeedsUpdateOfScreenEdgesDeferringSystemGestures() }
    }
}

/// react-native-screens has no option for deferring system edge gestures.
/// Screens that hide the home indicator (the live session screens) also defer
/// the bottom edge, so the first swipe up only shows the indicator instead of
/// leaving the app. UIKit asks a full-screen modal for this itself, and the
/// preference goes away with the screen when it is dismissed.
enum ScreenEdgeDeferral {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        let selector = #selector(getter: UIViewController.preferredScreenEdgesDeferringSystemGestures)
        guard let screenClass = NSClassFromString("RNSScreen"),
              let base = class_getInstanceMethod(UIViewController.self, selector) else {
            RsLog.marker("EDGES", ["install": "missing"])
            return
        }
        let edges: @convention(block) (UIViewController) -> UIRectEdge = { controller in
            controller.prefersHomeIndicatorAutoHidden ? .bottom : []
        }
        // Adds the getter to RNSScreen only; fails if the library ever defines it.
        let added = class_addMethod(screenClass, selector, imp_implementationWithBlock(edges),
                                    method_getTypeEncoding(base))
        RsLog.marker("EDGES", ["install": added ? "ok" : "exists"])
    }
}
