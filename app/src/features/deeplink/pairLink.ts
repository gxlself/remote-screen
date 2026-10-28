export type PairLinkDecision = 'wait' | 'go';

export function isPairLink(url: string): boolean {
  return /^rsp:\/\/pair\?/i.test(url);
}

export function decidePairLink(input: {
  onboardingDone: boolean;
  locked: boolean;
  topRoute?: string | null;
}): PairLinkDecision {
  if (!input.onboardingDone || input.locked) {
    return 'wait';
  }

  const route = input.topRoute ?? '';
  // Never on top of a live or just-ended session: the link waits until the
  // user leaves it.
  if (
    route === 'Session' ||
    route === 'AdapterSession' ||
    route === 'SessionEnded' ||
    route === 'Terminal' ||
    route === 'PairWaiting' ||
    route.startsWith('Onboard')
  ) {
    return 'wait';
  }
  return 'go';
}
