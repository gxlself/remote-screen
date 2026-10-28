import type { Computer } from '../../native/rscore/types';

// eslint-disable-next-line no-bitwise
export const AUDIO_OPUS_BIT = 1 << 15;
export const AUDIO_MIN_HOST = '0.4.0';

export type AudioAvailability = 'ok' | 'needsHost' | 'unknown';

export function audioAvailability(
  host: Computer | undefined,
): AudioAvailability {
  if (host === undefined) {
    return 'unknown';
  }
  // eslint-disable-next-line no-bitwise
  return (host.features & AUDIO_OPUS_BIT) !== 0 ? 'ok' : 'needsHost';
}
