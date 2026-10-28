import type { CoreEventOf } from '../../native/rscore/types';

export const PREWARM_PROBE_ID = 'prewarm';
const MAX_EVENTS_PER_PROBE = 64;
const MAX_PROBES = 8;

export type ProbeResultEvent = CoreEventOf<'probe.result'>;

/**
 * Buffers probe results that arrive while the native call is still resolving.
 * The default limits are 64 events per probe and 8 probe ids in total.
 */
export class EarlyProbeEvents {
  private readonly events = new Map<string, ProbeResultEvent[]>();

  push(event: ProbeResultEvent): void {
    if (event.probeId === '' || event.probeId === PREWARM_PROBE_ID) {
      return;
    }

    let queue = this.events.get(event.probeId);
    if (queue === undefined) {
      if (this.events.size >= MAX_PROBES) {
        const oldest = this.events.keys().next().value;
        if (oldest !== undefined) {
          this.events.delete(oldest);
        }
      }
      queue = [];
      this.events.set(event.probeId, queue);
    }

    if (queue.length >= MAX_EVENTS_PER_PROBE) {
      queue.shift();
    }
    queue.push(event);
  }

  take(probeId: string): ProbeResultEvent[] {
    const queue = this.events.get(probeId);
    if (queue === undefined) {
      return [];
    }
    this.events.delete(probeId);
    return queue;
  }

  clear(): void {
    this.events.clear();
  }
}
