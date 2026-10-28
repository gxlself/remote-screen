import { pipEligibility } from './eligibility';

it.each([
  [
    'viewer streaming with dimensions',
    {
      role: 'viewer' as const,
      overlay: 'none',
      phase: 'streaming',
      width: 1920,
      height: 1080,
    },
    { canStart: true, canContinue: true },
  ],
  [
    'controller',
    {
      role: 'controller' as const,
      overlay: 'none',
      phase: 'streaming',
      width: 1920,
      height: 1080,
    },
    { canStart: false, canContinue: false },
  ],
  [
    'viewer reconnecting',
    {
      role: 'viewer' as const,
      overlay: 'none',
      phase: 'reconnecting',
      width: 1920,
      height: 1080,
    },
    { canStart: false, canContinue: true },
  ],
  [
    'viewer with approval overlay',
    {
      role: 'viewer' as const,
      overlay: 'approval',
      phase: 'streaming',
      width: 1920,
      height: 1080,
    },
    { canStart: false, canContinue: true },
  ],
  [
    'zero width',
    {
      role: 'viewer' as const,
      overlay: 'none',
      phase: 'streaming',
      width: 0,
      height: 1080,
    },
    { canStart: false, canContinue: true },
  ],
  [
    'undefined phase',
    {
      role: 'viewer' as const,
      overlay: 'none',
      phase: undefined,
      width: 1920,
      height: 1080,
    },
    { canStart: false, canContinue: false },
  ],
] as const)('%s', (_name, input, expected) => {
  expect(pipEligibility(input)).toEqual(expected);
});
