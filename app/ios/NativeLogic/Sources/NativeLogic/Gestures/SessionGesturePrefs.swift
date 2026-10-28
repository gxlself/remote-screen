import CoreGraphics

struct SessionGesturePrefs: Equatable {
    let cursorSpeed: CGFloat
    let naturalScroll: Bool
    let haptics: Bool

    init(cursorSpeed: CGFloat = 1.4, naturalScroll: Bool = true, haptics: Bool = true) {
        self.cursorSpeed = cursorSpeed.isFinite ? min(max(cursorSpeed, 0.6), 2.4) : 1.4
        self.naturalScroll = naturalScroll
        self.haptics = haptics
    }

    func pointerDelta(_ d: CGVector) -> CGVector {
        let scale = cursorSpeed / 1.4
        return CGVector(dx: d.dx * scale, dy: d.dy * scale)
    }

    func scrollDelta(_ d: CGVector) -> CGVector {
        naturalScroll ? d : CGVector(dx: -d.dx, dy: -d.dy)
    }
}
