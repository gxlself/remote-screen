import { decidePairLink, isPairLink } from './pairLink';

describe('isPairLink', () => {
  it('accepts rsp pair links without regard to scheme case', () => {
    expect(isPairLink('RSP://pair?v=1')).toBe(true);
    expect(isPairLink('rsp://pair?v=1&h=host')).toBe(true);
  });

  it('rejects other hosts, paths, and schemes', () => {
    expect(isPairLink('rsp://pairx?v=1')).toBe(false);
    expect(isPairLink('https://pair?v=1')).toBe(false);
    expect(isPairLink('rsp://pair/path?v=1')).toBe(false);
  });
});

describe('decidePairLink', () => {
  it.each([
    ['onboarding incomplete', { onboardingDone: false, locked: false }],
    ['locked', { onboardingDone: true, locked: true }],
    ['session', { onboardingDone: true, locked: false, topRoute: 'Session' }],
    ['terminal', { onboardingDone: true, locked: false, topRoute: 'Terminal' }],
    [
      'screen sharing session',
      { onboardingDone: true, locked: false, topRoute: 'AdapterSession' },
    ],
    [
      'session ended page',
      { onboardingDone: true, locked: false, topRoute: 'SessionEnded' },
    ],
    [
      'pair waiting',
      { onboardingDone: true, locked: false, topRoute: 'PairWaiting' },
    ],
    [
      'onboarding route',
      { onboardingDone: true, locked: false, topRoute: 'OnboardProtect' },
    ],
  ])('waits when %s', (_label, input) => {
    expect(decidePairLink(input)).toBe('wait');
  });

  it('goes when onboarding is complete, unlocked, and idle', () => {
    expect(
      decidePairLink({
        onboardingDone: true,
        locked: false,
        topRoute: 'MainTabs',
      }),
    ).toBe('go');
  });
});
