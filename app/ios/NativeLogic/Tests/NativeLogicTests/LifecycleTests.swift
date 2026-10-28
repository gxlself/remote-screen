import XCTest
@testable import NativeLogic

final class LifecycleTests: XCTestCase {
    func testBackgroundBudgetLeavesHeadroomAndCapsAtThirtyTwoSeconds() {
        XCTAssertEqual(LifecycleBackgroundBudget.holdUntil(remaining: 29.5), 28.5)
        XCTAssertEqual(LifecycleBackgroundBudget.holdUntil(remaining: 180), 32)
        XCTAssertEqual(LifecycleBackgroundBudget.holdUntil(remaining: .infinity), 32)
    }

    func testPowerPolicyLowPowerModeAndBatteryThreshold() {
        XCTAssertTrue(
            LifecyclePowerPolicy.state(
                lowPowerMode: true,
                level: 0.5,
                batteryState: .unplugged
            ).lowPower
        )
        XCTAssertTrue(
            LifecyclePowerPolicy.state(
                lowPowerMode: false,
                level: 0.19,
                batteryState: .unplugged
            ).lowPower
        )
        XCTAssertFalse(
            LifecyclePowerPolicy.state(
                lowPowerMode: false,
                level: 0.19,
                batteryState: .charging
            ).lowPower
        )
        XCTAssertFalse(
            LifecyclePowerPolicy.state(
                lowPowerMode: false,
                level: 0.5,
                batteryState: .unplugged
            ).lowPower
        )
        XCTAssertFalse(
            LifecyclePowerPolicy.state(
                lowPowerMode: false,
                level: -1,
                batteryState: .unplugged
            ).lowPower
        )
    }

    func testPowerPolicyDebouncesSmallBatteryChangesAndTracksCharging() {
        let old = LifecyclePowerPolicy.state(
            lowPowerMode: false,
            level: 0.50,
            batteryState: .unplugged
        )
        let small = LifecyclePowerPolicy.state(
            lowPowerMode: false,
            level: 0.47,
            batteryState: .unplugged
        )
        let large = LifecyclePowerPolicy.state(
            lowPowerMode: false,
            level: 0.44,
            batteryState: .unplugged
        )
        let charging = LifecyclePowerPolicy.state(
            lowPowerMode: false,
            level: 0.50,
            batteryState: .charging
        )

        XCTAssertFalse(LifecyclePowerPolicy.changed(old, small))
        XCTAssertTrue(LifecyclePowerPolicy.changed(old, large))
        XCTAssertTrue(LifecyclePowerPolicy.changed(old, charging))
    }

    func testExitMarkerReportsBackgroundTerminationOnce() {
        let storage = InMemoryLifecycleExitMarkerStorage()
        let marker = LifecycleExitMarker(storage: storage)
        marker.didEnterBackground()
        XCTAssertTrue(marker.start())
        XCTAssertFalse(marker.start())
    }

    func testExitMarkerForegroundClearsBackgroundTermination() {
        let storage = InMemoryLifecycleExitMarkerStorage()
        let marker = LifecycleExitMarker(storage: storage)
        marker.didEnterBackground()
        marker.willEnterForeground()
        XCTAssertFalse(marker.start())
    }

    func testExitMarkerTerminateClearsBackgroundTermination() {
        let storage = InMemoryLifecycleExitMarkerStorage()
        let marker = LifecycleExitMarker(storage: storage)
        marker.didEnterBackground()
        marker.willTerminate()
        XCTAssertFalse(marker.start())
    }
}
