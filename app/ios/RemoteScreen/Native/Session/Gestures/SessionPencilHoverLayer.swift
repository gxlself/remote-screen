import UIKit

/// The local indicator for an Apple Pencil hovering over the session surface.
final class SessionPencilHoverLayer: CALayer {
    private static let diameter: CGFloat = 36

    override init() {
        super.init()
        anchorPoint = CGPoint(x: 0.5, y: 0.5)
        bounds = CGRect(x: 0, y: 0, width: Self.diameter, height: Self.diameter)
        cornerRadius = Self.diameter / 2
        borderWidth = 2
        borderColor = UIColor.white.cgColor
        backgroundColor = UIColor(red: 125.0 / 255.0, green: 168.0 / 255.0,
                                  blue: 255.0 / 255.0, alpha: 0.12).cgColor
        shadowColor = UIColor(red: 125.0 / 255.0, green: 168.0 / 255.0,
                              blue: 255.0 / 255.0, alpha: 1).cgColor
        shadowOpacity = 0.35
        shadowRadius = 5
        shadowOffset = .zero
        zPosition = 30
        contentsScale = UIScreen.main.scale
        isHidden = true
    }

    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { return nil }

    func show(at viewPoint: CGPoint) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        position = viewPoint
        isHidden = false
        CATransaction.commit()
    }

    func hide() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        isHidden = true
        CATransaction.commit()
    }
}
