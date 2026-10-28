import CoreGraphics
import Foundation

/// Which way a three-finger swipe goes between connected computers. Like the
/// spaces on a Mac: fingers moving left bring in the computer on the right.
public enum SpaceSwipeDirection: String, Equatable {
    case previous
    case next
}

/// Pure decisions behind the three-finger "switch computer" swipe:
/// telling a swipe from a tap, how far the picture follows the fingers, and
/// whether letting go switches or springs back.
public enum SpaceSwipe {
    /// Points the fingers travel before a touch counts as a swipe. Below this
    /// the three-finger tap (toolbar) still gets its chance.
    public static let slop: CGFloat = 24
    /// Horizontal travel must beat vertical travel by this factor.
    public static let horizontalBias: CGFloat = 1.2
    /// Past this share of the width a slow release still switches.
    public static let commitFraction: CGFloat = 1.0 / 3.0
    /// A flick at this speed (points per second) switches from any distance
    /// past the slop.
    public static let flickVelocity: CGFloat = 600
    /// How hard the edge resists when there is no computer on that side.
    public static let rubberBand: CGFloat = 0.55

    public enum Classification: Equatable {
        /// Not moved far enough to tell yet.
        case undecided
        /// A horizontal swipe: take over the touch.
        case swipe
        /// Mostly vertical: leave the touch to the other gestures.
        case other
    }

    public static func classify(translation: CGPoint) -> Classification {
        let dx = abs(translation.x)
        let dy = abs(translation.y)
        if dx < slop, dy < slop { return .undecided }
        return dx > dy * horizontalBias ? .swipe : .other
    }

    /// Where the current picture sits while following `translationX`.
    /// Positive moves it right and brings in the previous computer.
    public static func offset(translationX: CGFloat, width: CGFloat,
                              hasPrevious: Bool, hasNext: Bool) -> CGFloat {
        guard width > 0, translationX.isFinite else { return 0 }
        let open = translationX > 0 ? hasPrevious : hasNext
        if open {
            return max(-width, min(width, translationX))
        }
        let pulled = abs(translationX)
        let resisted = (1 - 1 / (pulled * rubberBand / width + 1)) * width
        return translationX > 0 ? resisted : -resisted
    }

    /// What letting go does: switch to the neighbour, or nil to spring back.
    public static func decide(translationX: CGFloat, velocityX: CGFloat, width: CGFloat,
                              hasPrevious: Bool, hasNext: Bool) -> SpaceSwipeDirection? {
        guard width > 0, translationX.isFinite, velocityX.isFinite,
              abs(translationX) >= slop else { return nil }
        let direction: SpaceSwipeDirection = translationX > 0 ? .previous : .next
        guard direction == .previous ? hasPrevious : hasNext else { return nil }
        let far = abs(translationX) >= width * commitFraction
        // A flick counts only when it goes the same way as the drag.
        let flick = abs(velocityX) >= flickVelocity && (velocityX > 0) == (translationX > 0)
        // Flung back toward the start: spring back even past the third.
        let flungBack = abs(velocityX) >= flickVelocity && (velocityX > 0) != (translationX > 0)
        return (far && !flungBack) || flick ? direction : nil
    }

    /// The neighbours of `current` in `ids` (no wrap-around, like a Mac).
    public static func neighbours(of current: String, in ids: [String]) -> (previous: String?, next: String?) {
        guard let index = ids.firstIndex(of: current) else { return (nil, nil) }
        let previous = index > 0 ? ids[index - 1] : nil
        let next = index + 1 < ids.count ? ids[index + 1] : nil
        return (previous, next)
    }

    /// Where the current picture ends up once the switch animation is over.
    public static func finalOffset(_ direction: SpaceSwipeDirection?, width: CGFloat) -> CGFloat {
        switch direction {
        case .previous?: return width
        case .next?: return -width
        case nil: return 0
        }
    }
}
