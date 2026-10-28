export type PipEligibility = { canStart: boolean; canContinue: boolean };

export function pipEligibility(input: {
  role: 'controller' | 'viewer' | null | undefined;
  overlay: string;
  phase: string | undefined;
  width: number;
  height: number;
}): PipEligibility {
  const canStart =
    input.role === 'viewer' &&
    input.overlay === 'none' &&
    input.phase === 'streaming' &&
    input.width > 0 &&
    input.height > 0;
  const canContinue =
    input.role === 'viewer' &&
    (input.phase === 'streaming' || input.phase === 'reconnecting');
  return { canStart, canContinue };
}
