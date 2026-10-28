import {
  LOCK_AFTER_BACKGROUND_MS,
  lockOnLaunch,
  lockOnResume,
  needsAuthToChange,
} from './policy';

const NOW = 1_792_465_134_000;

describe('lockOnLaunch', () => {
  it.each([
    [true, true, true],
    [true, false, false],
    [false, true, false],
    [false, false, false],
  ])(
    'onboardingDone=%s onOpen=%s returns %s',
    (onboardingDone, onOpen, expected) => {
      expect(lockOnLaunch({ onboardingDone, onOpen })).toBe(expected);
    },
  );
});

describe('lockOnResume', () => {
  const base = { onboardingDone: true, onOpen: true, nowMs: NOW };

  it('does not lock before the five minute boundary', () => {
    expect(
      lockOnResume({
        ...base,
        backgroundAtMs: NOW - (LOCK_AFTER_BACKGROUND_MS - 1000),
      }),
    ).toBe(false);
  });

  it('locks at exactly five minutes', () => {
    expect(
      lockOnResume({
        ...base,
        backgroundAtMs: NOW - LOCK_AFTER_BACKGROUND_MS,
      }),
    ).toBe(true);
  });

  it('does not lock without a background timestamp', () => {
    expect(lockOnResume({ ...base, backgroundAtMs: null })).toBe(false);
  });

  it('does not lock when opening verification is disabled', () => {
    expect(
      lockOnResume({
        ...base,
        onOpen: false,
        backgroundAtMs: NOW - LOCK_AFTER_BACKGROUND_MS,
      }),
    ).toBe(false);
  });

  it('does not lock before onboarding is done', () => {
    expect(
      lockOnResume({
        ...base,
        onboardingDone: false,
        backgroundAtMs: NOW - LOCK_AFTER_BACKGROUND_MS,
      }),
    ).toBe(false);
  });
});

describe('needsAuthToChange', () => {
  it('only requires authentication when disabling protection', () => {
    expect(needsAuthToChange(true, false)).toBe(true);
    expect(needsAuthToChange(true, true)).toBe(false);
    expect(needsAuthToChange(false, true)).toBe(false);
    expect(needsAuthToChange(false, false)).toBe(false);
  });
});
