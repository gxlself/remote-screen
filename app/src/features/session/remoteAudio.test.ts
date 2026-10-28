import type { Computer } from '../../native/rscore/types';
import { AUDIO_OPUS_BIT, audioAvailability } from './remoteAudio';

function host(features: number): Computer {
  return { features } as Computer;
}

it('reports hosts that advertise AUDIO_OPUS as ready', () => {
  expect(audioAvailability(host(AUDIO_OPUS_BIT))).toBe('ok');
});

it('requires Host 0.4.0 when the audio bit is absent', () => {
  expect(audioAvailability(host(0))).toBe('needsHost');
});

it('ignores unrelated feature bits', () => {
  expect(audioAvailability(host(AUDIO_OPUS_BIT + 3))).toBe('ok');
});

it('treats an unresolved host as unknown and usable', () => {
  expect(audioAvailability(undefined)).toBe('unknown');
});
