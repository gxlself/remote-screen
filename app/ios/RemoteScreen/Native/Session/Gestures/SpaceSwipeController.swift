import UIKit

/// One neighbouring computer drawn beside the picture while swiping: its last
/// picture, or a dark tile with its name when there is none.
final class SpaceTileLayer: CALayer {
    private let picture = CALayer()
    private let label = CATextLayer()

    override init() {
        super.init()
        backgroundColor = UIColor(white: 0.07, alpha: 1).cgColor
        masksToBounds = true
        picture.contentsGravity = .resizeAspect
        addSublayer(picture)
        label.alignmentMode = .center
        label.fontSize = 22
        label.foregroundColor = UIColor(white: 1, alpha: 0.72).cgColor
        label.truncationMode = .end
        label.contentsScale = UIScreen.main.scale
        addSublayer(label)
        isHidden = true
    }

    override init(layer: Any) { super.init(layer: layer) }

    required init?(coder: NSCoder) { return nil }

    func show(image: CGImage?, name: String) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        picture.contents = image
        picture.isHidden = image == nil
        label.string = name
        label.isHidden = image != nil
        isHidden = false
        CATransaction.commit()
    }

    func clear() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        picture.contents = nil
        label.string = nil
        isHidden = true
        CATransaction.commit()
    }

    override func layoutSublayers() {
        super.layoutSublayers()
        picture.frame = bounds
        let height: CGFloat = 30
        label.frame = CGRect(x: 16, y: (bounds.height - height) / 2, width: max(0, bounds.width - 32), height: height)
    }
}

/// Three fingers left or right move between the computers that are connected,
/// like the spaces on a Mac. The picture follows the fingers with
/// the neighbours' last pictures sliding in; letting go switches or springs
/// back. The page is told only when to switch, after the slide has finished.
final class SpaceSwipeController: NSObject, UIGestureRecognizerDelegate {
    private weak var surface: RemoteSessionSurface?
    private let pan: UIPanGestureRecognizer
    private let previousTile = SpaceTileLayer()
    private let nextTile = SpaceTileLayer()
    private var ids: [String] = []
    private var names: [String] = []
    private var swiping = false
    /// Waiting for the page to replace itself after a switch.
    private var committed = false
    private var commitGeneration = 0
    private static let duration: CFTimeInterval = 0.28
    /// The page normally replaces itself well within this; if it does not
    /// (the target ended meanwhile), the picture slides back.
    private static let replaceTimeout: TimeInterval = 1.5

    var onSwipe: ((String) -> Void)?
    var onSwitch: ((String) -> Void)?

    init(surface: RemoteSessionSurface) {
        self.surface = surface
        pan = UIPanGestureRecognizer()
        super.init()
        pan.addTarget(self, action: #selector(onPan(_:)))
        pan.minimumNumberOfTouches = 3
        pan.maximumNumberOfTouches = 3
        pan.delegate = self
        pan.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
        surface.layer.addSublayer(previousTile)
        surface.layer.addSublayer(nextTile)
        surface.installSpaceRecognizer(pan)
    }

    func setSpaces(ids: [String], names: [String]) {
        self.ids = ids
        self.names = names
    }

    func setSuspended(_ suspended: Bool) {
        if suspended, swiping { springBack(animated: false) }
        pan.isEnabled = !suspended
    }

    func layout(bounds: CGRect) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        previousTile.frame = bounds.offsetBy(dx: -bounds.width, dy: 0)
        nextTile.frame = bounds.offsetBy(dx: bounds.width, dy: 0)
        CATransaction.commit()
    }

    /// Back to the plain picture (the view is reused for another session).
    func reset() {
        swiping = false
        committed = false
        commitGeneration += 1
        surface?.layer.removeAnimation(forKey: "space")
        setOffset(0)
        previousTile.clear()
        nextTile.clear()
    }

    /// ⌃⌥⌘ keys: the same slide toward `target`, then the switch.
    func animate(to target: String, direction: SpaceSwipeDirection) {
        // Already sliding toward a switch: that one wins.
        guard let surface, !committed else { return }
        guard !swiping, surface.bounds.width > 0 else {
            onSwitch?(target)
            return
        }
        surface.captureSnapshot()
        let tile = direction == .previous ? previousTile : nextTile
        tile.show(image: SessionSnapshotStore.shared.image(for: target), name: name(of: target))
        finish(direction: direction, target: target)
    }

    // MARK: - Gesture

    func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        guard let surface, surface.sessionInputReady, !committed else { return false }
        let velocity = pan.velocity(in: surface)
        let moved = pan.translation(in: surface)
        // Horizontal by speed now, or by distance once past the slop.
        if SpaceSwipe.classify(translation: moved) == .other { return false }
        return abs(velocity.x) > abs(velocity.y) * SpaceSwipe.horizontalBias
    }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }

    /// The three-finger tap (toolbar) waits until this is not a swipe.
    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
        guard let tap = other as? UITapGestureRecognizer else { return false }
        return tap.numberOfTouchesRequired == 3
    }

    @objc private func onPan(_ recognizer: UIPanGestureRecognizer) {
        guard let surface else { return }
        let width = surface.bounds.width
        let translation = recognizer.translation(in: surface).x
        let (previous, next) = neighbours()
        switch recognizer.state {
        case .began:
            swiping = true
            // Whatever the fingers started on the remote ends here.
            surface.cancelInputForSpaceSwipe()
            surface.captureSnapshot()
            if let previous { previousTile.show(image: image(of: previous), name: name(of: previous)) }
            if let next { nextTile.show(image: image(of: next), name: name(of: next)) }
            onSwipe?("began")
            RsLog.marker("SPACES", ["swipe": "began", "count": ids.count])
            setOffset(SpaceSwipe.offset(translationX: translation, width: width,
                                        hasPrevious: previous != nil, hasNext: next != nil))
        case .changed:
            setOffset(SpaceSwipe.offset(translationX: translation, width: width,
                                        hasPrevious: previous != nil, hasNext: next != nil))
        case .ended:
            let velocity = recognizer.velocity(in: surface).x
            let direction = SpaceSwipe.decide(translationX: translation, velocityX: velocity, width: width,
                                               hasPrevious: previous != nil, hasNext: next != nil)
            swiping = false
            onSwipe?("ended")
            if let direction, let target = direction == .previous ? previous : next {
                finish(direction: direction, target: target)
            } else {
                springBack(animated: true)
            }
        case .cancelled, .failed:
            swiping = false
            onSwipe?("ended")
            springBack(animated: true)
        default:
            break
        }
    }

    // MARK: - Private

    private func neighbours() -> (previous: String?, next: String?) {
        guard let current = surface?.sessionKey else { return (nil, nil) }
        return SpaceSwipe.neighbours(of: current, in: ids)
    }

    private func image(of id: String) -> CGImage? { SessionSnapshotStore.shared.image(for: id) }

    private func name(of id: String) -> String {
        guard let index = ids.firstIndex(of: id), index < names.count else { return "" }
        return names[index]
    }

    private func currentOffset() -> CGFloat {
        guard let layer = surface?.layer else { return 0 }
        let transform = layer.presentation()?.sublayerTransform ?? layer.sublayerTransform
        return transform.m41
    }

    private func setOffset(_ offset: CGFloat) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        surface?.layer.sublayerTransform = CATransform3DMakeTranslation(offset, 0, 0)
        CATransaction.commit()
    }

    private func finish(direction: SpaceSwipeDirection, target: String) {
        guard let surface else { return }
        committed = true
        commitGeneration += 1
        let generation = commitGeneration
        RsLog.marker("SPACES", ["switch": direction.rawValue])
        slide(to: SpaceSwipe.finalOffset(direction, width: surface.bounds.width)) { [weak self] in
            guard let self, self.commitGeneration == generation else { return }
            // The page replaces itself; the target's picture stays in view
            // until then, so the new page opens on the same picture.
            self.onSwitch?(target)
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.replaceTimeout) { [weak self] in
                guard let self, self.committed, self.commitGeneration == generation else { return }
                RsLog.marker("SPACES", ["switch": "notReplaced"])
                self.committed = false
                self.springBack(animated: true)
            }
        }
    }

    private func springBack(animated: Bool) {
        slide(to: 0, animated: animated) { [weak self] in
            guard let self, !self.swiping, !self.committed else { return }
            self.previousTile.clear()
            self.nextTile.clear()
        }
    }

    private func slide(to offset: CGFloat, animated: Bool = true, completion: @escaping () -> Void) {
        guard let layer = surface?.layer else { return }
        let reduceMotion = surface?.reduceMotion == true || UIAccessibility.isReduceMotionEnabled
        let from = currentOffset()
        layer.removeAnimation(forKey: "space")
        setOffset(offset)
        guard animated, !reduceMotion, abs(from - offset) > 0.5 else {
            completion()
            return
        }
        CATransaction.begin()
        CATransaction.setCompletionBlock(completion)
        let animation = CABasicAnimation(keyPath: "sublayerTransform")
        animation.fromValue = NSValue(caTransform3D: CATransform3DMakeTranslation(from, 0, 0))
        animation.toValue = NSValue(caTransform3D: CATransform3DMakeTranslation(offset, 0, 0))
        animation.duration = Self.duration
        animation.timingFunction = CAMediaTimingFunction(name: .easeOut)
        layer.add(animation, forKey: "space")
        CATransaction.commit()
    }
}
