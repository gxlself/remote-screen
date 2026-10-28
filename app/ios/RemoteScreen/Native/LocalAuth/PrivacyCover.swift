import UIKit

@objc(PrivacyCover)
final class PrivacyCover: NSObject {
    @objc(shared)
    static let shared = PrivacyCover()

    private var isEnabled = false
    private var isObserving = false
    private var coverView: UIView?
    private weak var coverWindow: UIWindow?

    private override init() {}

    @objc(setEnabled:)
    func setEnabled(_ enabled: Bool) {
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in
                self?.setEnabled(enabled)
            }
            return
        }
        isEnabled = enabled
        if enabled {
            startObserving()
        } else {
            stopObserving()
            removeCover(animated: false)
        }
    }

    private func startObserving() {
        guard !isObserving else { return }
        isObserving = true
        let center = NotificationCenter.default
        center.addObserver(
            self,
            selector: #selector(sceneWillDeactivate),
            name: UIScene.willDeactivateNotification,
            object: nil
        )
        center.addObserver(
            self,
            selector: #selector(sceneDidActivate),
            name: UIScene.didActivateNotification,
            object: nil
        )
    }

    private func stopObserving() {
        guard isObserving else { return }
        isObserving = false
        NotificationCenter.default.removeObserver(
            self,
            name: UIScene.willDeactivateNotification,
            object: nil
        )
        NotificationCenter.default.removeObserver(
            self,
            name: UIScene.didActivateNotification,
            object: nil
        )
    }

    @objc private func sceneWillDeactivate() {
        guard isEnabled else { return }
        showCover()
    }

    @objc private func sceneDidActivate() {
        guard isEnabled else { return }
        removeCover(animated: true)
    }

    private func showCover() {
        guard let window = keyWindow() else { return }
        if let coverView, coverWindow === window {
            window.bringSubviewToFront(coverView)
            coverView.layer.removeAllAnimations()
            coverView.alpha = 1
            return
        }
        removeCover(animated: false)

        let cover = makeCoverView()
        cover.frame = window.bounds
        cover.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        window.addSubview(cover)
        window.bringSubviewToFront(cover)
        coverView = cover
        coverWindow = window
    }

    private func removeCover(animated: Bool) {
        guard let cover = coverView else { return }
        let finish = { [weak self, weak cover] in
            guard let self, self.coverView === cover else { return }
            cover?.removeFromSuperview()
            self.coverView = nil
            self.coverWindow = nil
        }
        if animated && !UIAccessibility.isReduceMotionEnabled {
            UIView.animate(
                withDuration: 0.15,
                animations: { cover.alpha = 0 },
                completion: { _ in finish() }
            )
        } else {
            finish()
        }
    }

    private func keyWindow() -> UIWindow? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        return scenes
            .flatMap(\.windows)
            .first(where: \.isKeyWindow)
            ?? scenes.first?.windows.first(where: { !$0.isHidden && $0.alpha > 0 })
    }

    private func makeCoverView() -> UIView {
        let view = UIView()
        view.backgroundColor = UIColor(
            red: 0x0B / 255,
            green: 0x0E / 255,
            blue: 0x10 / 255,
            alpha: 1
        )
        view.isAccessibilityElement = true
        view.accessibilityLabel = "Remote Screen"

        let imageView = UIImageView(
            image: UIImage(named: "AppIcon60x60") ?? UIImage(named: "AppIcon-1024")
        )
        imageView.contentMode = .scaleAspectFit
        imageView.translatesAutoresizingMaskIntoConstraints = false
        imageView.widthAnchor.constraint(equalToConstant: 64).isActive = true
        imageView.heightAnchor.constraint(equalToConstant: 64).isActive = true

        let label = UILabel()
        label.text = "Remote Screen"
        label.textColor = UIColor(red: 0xEC / 255, green: 0xF0 / 255, blue: 0xF2 / 255, alpha: 1)
        label.font = .systemFont(ofSize: 15, weight: .semibold)
        label.textAlignment = .center

        let stack = UIStackView(arrangedSubviews: [imageView, label])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        return view
    }
}
