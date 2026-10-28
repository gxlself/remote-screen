import type { CoreEventOf } from '../../native/rscore/types';
import { EarlyProbeEvents, PREWARM_PROBE_ID } from './earlyProbeEvents';

const HOST_ID = 'w3k8p2x7m4q9tb6c';

function event(
  probeId: string,
  index: number,
  last = false,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId: HOST_ID,
    path: 'intranet',
    addr: `10.30.12.${index + 1}`,
    port: 7400,
    transport: 'quic',
    outcome: 'ok',
    ms: index,
    last,
  };
}

it('takes only one probe id and preserves arrival order', () => {
  const buffer = new EarlyProbeEvents();
  const first = event('p1', 1);
  const second = event('p2', 2);
  const third = event('p1', 3);
  buffer.push(first);
  buffer.push(second);
  buffer.push(third);

  expect(buffer.take('p1')).toEqual([first, third]);
  expect(buffer.take('p2')).toEqual([second]);
  expect(buffer.take('p1')).toEqual([]);
});

it('does not buffer prewarm or empty probe ids', () => {
  const buffer = new EarlyProbeEvents();
  buffer.push(event(PREWARM_PROBE_ID, 1));
  buffer.push(event('', 2));

  expect(buffer.take(PREWARM_PROBE_ID)).toEqual([]);
  expect(buffer.take('')).toEqual([]);
});

it('keeps the newest 64 events for a probe and newest 8 probe ids', () => {
  const buffer = new EarlyProbeEvents();
  for (let index = 0; index < 65; index += 1) {
    buffer.push(event('p1', index));
  }
  const events = buffer.take('p1');
  expect(events).toHaveLength(64);
  expect(events[0]?.ms).toBe(1);
  expect(events[63]?.ms).toBe(64);

  for (let index = 1; index <= 9; index += 1) {
    buffer.push(event(`id-${index}`, index));
  }
  expect(buffer.take('id-1')).toEqual([]);
  expect(buffer.take('id-2')).toEqual([event('id-2', 2)]);
  expect(buffer.take('id-9')).toEqual([event('id-9', 9)]);
});

it('clears every buffered event', () => {
  const buffer = new EarlyProbeEvents();
  buffer.push(event('p1', 1));
  buffer.push(event('p2', 2));
  buffer.clear();

  expect(buffer.take('p1')).toEqual([]);
  expect(buffer.take('p2')).toEqual([]);
});
