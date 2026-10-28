import { useCallback, useEffect, useRef, useState } from 'react';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { useCore } from '../../native/rscore/CoreContext';
import type {
  CoreEventOf,
  DiscoverKind,
  DiscoverOutcome,
} from '../../native/rscore/types';
import { useCoreEvent } from '../../features/events/EventsProvider';
import type {
  AddrProbe,
  NativeKind,
  ProbeKind,
} from '../../features/servers/addressProbe';
import {
  IDLE_PROBE,
  RDP_PORT,
  VNC_PORT,
  applyDiscover,
  applyProbe,
  settleProbe,
  startProbe,
  timeoutProbe,
} from '../../features/servers/addressProbe';

export const ADDRESS_PROBE_TIMEOUT_MS = 5000;
/** Per-port budget for the discover 5900 / 3389 / 22 probes (V10 C2). */
export const NATIVE_PROBE_TIMEOUT_MS = 3000;

type ProbeTarget = { kind: ProbeKind; port: number };
type DiscoverTarget = {
  kind: NativeKind | 'ssh';
  port: number;
  probe: DiscoverKind;
};

const DISCOVER_TARGETS: DiscoverTarget[] = [
  { kind: 'rdp', port: RDP_PORT, probe: 'rdp' },
  { kind: 'vnc', port: VNC_PORT, probe: 'vnc' },
  // A plain TCP connect: sshd never answers the RSP dial.
  { kind: 'ssh', port: 22, probe: 'tcp' },
];

function skippedResult(
  host: string,
  port: number,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId: 'unsupported',
    hostId: null,
    path: 'manual',
    addr: host,
    port,
    transport: 'tcp',
    outcome: 'skipped',
    ms: null,
    reason: 'unsupported',
    last: true,
  };
}

function isUnsupported(error: unknown): boolean {
  return error instanceof RsCoreError && error.kind === 'unsupported';
}

export function useAddressProbe(): {
  probe: AddrProbe;
  start: (host: string, port: number) => void;
  /** Drops the result; a probe still in flight is ignored. */
  reset: () => void;
} {
  const core = useCore();
  const [probe, setProbe] = useState<AddrProbe>(IDLE_PROBE);
  const generation = useRef(0);
  const probeKinds = useRef(new Map<string, ProbeKind>());
  const earlyEvents = useRef(new Map<string, CoreEventOf<'probe.result'>[]>());
  const timeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = useCallback(() => {
    if (timeout.current !== null) {
      clearTimeout(timeout.current);
      timeout.current = null;
    }
  }, []);

  const applyEvents = useCallback(
    (
      current: AddrProbe,
      kind: ProbeKind,
      events: CoreEventOf<'probe.result'>[],
    ): AddrProbe => {
      const nowMs = Date.now();
      return events.reduce(
        (state, event) => applyProbe(state, kind, event, nowMs),
        current,
      );
    },
    [],
  );

  useCoreEvent('probe.result', event => {
    const kind = probeKinds.current.get(event.probeId);
    if (kind === undefined) {
      const buffered = earlyEvents.current.get(event.probeId) ?? [];
      earlyEvents.current.set(event.probeId, [...buffered, event]);
      return;
    }
    setProbe(current => applyEvents(current, kind, [event]));
  });

  useEffect(() => {
    if (probe.state === 'done') {
      clearTimer();
    }
  }, [clearTimer, probe.state]);

  useEffect(() => {
    return () => {
      generation.current += 1;
      clearTimer();
    };
  }, [clearTimer]);

  const start = useCallback(
    (host: string, port: number) => {
      clearTimer();
      generation.current += 1;
      const runGeneration = generation.current;
      probeKinds.current.clear();
      earlyEvents.current.clear();
      const startedAtMs = Date.now();
      // Older native packages have no discover binding: 3389 and 22 keep the
      // RSP dial and 5900 is not probed.
      const discover = core.discover;
      setProbe(startProbe(startedAtMs, discover !== undefined));

      timeout.current = setTimeout(() => {
        timeout.current = null;
        if (generation.current === runGeneration) {
          setProbe(current => timeoutProbe(current, Date.now()));
        }
      }, ADDRESS_PROBE_TIMEOUT_MS);

      const targets: ProbeTarget[] =
        discover === undefined
          ? [
              { kind: 'rs', port },
              { kind: 'rdp', port: RDP_PORT },
              { kind: 'ssh', port: 22 },
            ]
          : [{ kind: 'rs', port }];

      if (discover !== undefined) {
        for (const target of DISCOVER_TARGETS) {
          const applyNative = (outcome: DiscoverOutcome) => {
            if (generation.current === runGeneration) {
              setProbe(current =>
                applyDiscover(current, target.kind, outcome, Date.now()),
              );
            }
          };
          discover
            .probe(host, target.port, target.probe, NATIVE_PROBE_TIMEOUT_MS)
            .then(
              result => applyNative(result.outcome),
              () => applyNative('timeout'),
            );
        }
      }

      const run = async () => {
        for (const target of targets) {
          if (generation.current !== runGeneration) {
            return;
          }
          try {
            const probeId = await core.diag.probeAddress(host, target.port);
            if (generation.current !== runGeneration) {
              return;
            }
            probeKinds.current.set(probeId, target.kind);
            const buffered = earlyEvents.current.get(probeId);
            if (buffered !== undefined) {
              earlyEvents.current.delete(probeId);
              setProbe(current => applyEvents(current, target.kind, buffered));
            }
          } catch (error) {
            if (generation.current !== runGeneration) {
              return;
            }
            if (isUnsupported(error)) {
              // Native discover probes, when present, still settle the result.
              setProbe(current =>
                settleProbe(
                  {
                    ...current,
                    rs: skippedResult(host, port),
                    hello: null,
                    rdp: current.discover
                      ? current.rdp
                      : current.rdp ?? 'closed',
                    ssh: current.discover
                      ? current.ssh
                      : current.ssh ?? 'closed',
                  },
                  Date.now(),
                ),
              );
              probeKinds.current.clear();
              return;
            }
            const fake: CoreEventOf<'probe.result'> = {
              type: 'probe.result',
              probeId: `failed-${target.kind}`,
              hostId: null,
              path: 'manual',
              addr: host,
              port: target.port,
              transport: 'tcp',
              outcome: 'noRoute',
              ms: null,
              reason:
                error instanceof Error ? error.message : 'probe request failed',
              last: true,
            };
            setProbe(current =>
              applyProbe(current, target.kind, fake, Date.now()),
            );
          }
        }
      };

      run().catch(() => {
        // Every per-port failure is converted into a probe result above.
      });
    },
    [applyEvents, clearTimer, core],
  );

  const reset = useCallback(() => {
    clearTimer();
    generation.current += 1;
    probeKinds.current.clear();
    earlyEvents.current.clear();
    setProbe(IDLE_PROBE);
  }, [clearTimer]);

  return { probe, start, reset };
}
