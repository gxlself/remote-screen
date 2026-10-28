import UIKit

final class SessionCursorLayer: CALayer {
    private let arrow = CAShapeLayer()

    override init() {
        super.init()
        anchorPoint = .zero
        zPosition = 1
        contentsGravity = .resize
        contentsScale = UIScreen.main.scale
        let path = CGMutablePath()
        path.move(to: CGPoint(x: 0.5, y: 0.5))
        for point in [CGPoint(x: 0.5, y: 20.5), CGPoint(x: 5.3, y: 16),
                      CGPoint(x: 9, y: 23.3), CGPoint(x: 12, y: 21.8),
                      CGPoint(x: 8.3, y: 14.5), CGPoint(x: 15.2, y: 14.5)] {
            path.addLine(to: point)
        }
        path.closeSubpath()
        arrow.path = path
        arrow.fillColor = UIColor.white.cgColor
        arrow.strokeColor = UIColor.black.cgColor
        arrow.lineWidth = 1
        arrow.lineJoin = .round
        arrow.contentsScale = contentsScale
        addSublayer(arrow)
        isHidden = true
    }

    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { return nil }

    func update(image: CGImage?, size: CGSize, hotspot: CGPoint, tip: CGPoint, hidden: Bool) {
        precondition(Thread.isMainThread)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        bounds = CGRect(origin: .zero, size: size)
        arrow.frame = bounds
        arrow.isHidden = image != nil
        contents = image
        position = CGPoint(x: tip.x - hotspot.x, y: tip.y - hotspot.y)
        isHidden = hidden
        CATransaction.commit()
    }
}
