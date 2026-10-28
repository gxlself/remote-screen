package ai.gxlself.remotescreen.logic.lifecycle

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LifecyclePowerTest {
  @Test
  fun lowPowerModeAlwaysSetsLowPower() {
    assertTrue(PowerPolicy.state(true, 0.80, BatteryKind.UNPLUGGED).lowPower)
  }

  @Test
  fun lowBatteryIsLowPowerOnlyWhenUnplugged() {
    assertTrue(PowerPolicy.state(false, 0.19, BatteryKind.UNPLUGGED).lowPower)
    assertFalse(PowerPolicy.state(false, 0.19, BatteryKind.CHARGING).lowPower)
    assertFalse(PowerPolicy.state(false, 0.20, BatteryKind.UNPLUGGED).lowPower)
  }

  @Test
  fun unknownBatteryLevelDoesNotSetLowPower() {
    val state = PowerPolicy.state(false, -1.0, BatteryKind.UNKNOWN)
    assertFalse(state.lowPower)
    assertEquals(-1.0, state.batteryLevel, 0.0)
  }

  @Test
  fun chargingTracksFullAndChargingKinds() {
    assertTrue(PowerPolicy.state(false, 0.8, BatteryKind.CHARGING).charging)
    assertTrue(PowerPolicy.state(false, 1.0, BatteryKind.FULL).charging)
    assertFalse(PowerPolicy.state(false, 0.8, BatteryKind.UNKNOWN).charging)
  }

  @Test
  fun policyDebouncesSmallBatteryChanges() {
    val old = PowerState(false, false, 0.50, false)
    assertFalse(PowerPolicy.changed(old, old.copy(batteryLevel = 0.47)))
    assertTrue(PowerPolicy.changed(old, old.copy(batteryLevel = 0.44)))
    assertTrue(PowerPolicy.changed(old, old.copy(charging = true)))
  }

  @Test
  fun policyReportsUnknownLevelTransitions() {
    assertFalse(PowerPolicy.changed(PowerState(false, false, -1.0, false), PowerState(false, false, -1.0, false)))
    assertTrue(PowerPolicy.changed(PowerState(false, false, -1.0, false), PowerState(false, false, 0.5, false)))
  }

  @Test
  fun mapperMapsKnownBatteryStatuses() {
    assertEquals(BatteryKind.CHARGING, BatteryMapper.kind(2, 0))
    assertEquals(BatteryKind.FULL, BatteryMapper.kind(5, 0))
    assertEquals(BatteryKind.UNPLUGGED, BatteryMapper.kind(3, 0))
    assertEquals(BatteryKind.UNPLUGGED, BatteryMapper.kind(4, 0))
  }

  @Test
  fun mapperTreatsPluggedUnknownStatusAsCharging() {
    assertEquals(BatteryKind.CHARGING, BatteryMapper.kind(1, 1))
    assertEquals(BatteryKind.UNKNOWN, BatteryMapper.kind(1, 0))
  }

  @Test
  fun mapperClampsAndRejectsInvalidLevels() {
    assertEquals(-1.0, BatteryMapper.level(0, 100), 0.0)
    assertEquals(-1.0, BatteryMapper.level(20, 0), 0.0)
    assertEquals(0.5, BatteryMapper.level(50, 100), 0.0)
    assertEquals(1.0, BatteryMapper.level(150, 100), 0.0)
  }
}
