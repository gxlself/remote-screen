export const LOCK_AFTER_BACKGROUND_MS = 5 * 60 * 1000;

export function lockOnLaunch(s: {
  onboardingDone: boolean;
  onOpen: boolean;
}): boolean {
  return s.onboardingDone && s.onOpen;
}

export function lockOnResume(s: {
  onboardingDone: boolean;
  onOpen: boolean;
  backgroundAtMs: number | null;
  nowMs: number;
}): boolean {
  return (
    lockOnLaunch(s) &&
    s.backgroundAtMs !== null &&
    s.nowMs - s.backgroundAtMs >= LOCK_AFTER_BACKGROUND_MS
  );
}

export function needsAuthToChange(from: boolean, to: boolean): boolean {
  return from && !to;
}
