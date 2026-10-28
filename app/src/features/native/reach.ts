import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { entryProtocol } from './entries';
import type { ComputerEntry, EntryReach } from './types';

const REACH_INTERVAL_MS = 30_000;
const PROBE_TIMEOUT_MS = 800;
const MAX_CONCURRENT_PROBES = 8;

export type ManualReach = Record<string, EntryReach>;

type Discover = NonNullable<RsCoreApi['discover']>;
type Target = {
  key: string;
  host: string;
  port: number;
  kind: 'vnc' | 'rdp';
};
type ReachResult = {
  target: Target;
  discover: Discover;
  reach: EntryReach;
};

/**
 * The last answer per address, shared by every list, so a sheet opened from
 * Home starts from Home's result instead of "unknown" (H34).
 */
const lastReach = new WeakMap<Discover, Map<string, EntryReach>>();

function targetId(target: Target): string {
  return `${target.kind}|${target.host}|${target.port}`;
}

function remembered(
  discover: Discover | undefined,
  target: Target,
): EntryReach | undefined {
  return discover === undefined
    ? undefined
    : lastReach.get(discover)?.get(targetId(target));
}

function remember(discover: Discover, target: Target, reach: EntryReach) {
  let known = lastReach.get(discover);
  if (known === undefined) {
    known = new Map();
    lastReach.set(discover, known);
  }
  known.set(targetId(target), reach);
}

function sameTarget(left: Target, right: Target): boolean {
  return (
    left.host === right.host &&
    left.port === right.port &&
    left.kind === right.kind
  );
}

async function probeTarget(
  target: Target,
  discover: Discover,
): Promise<EntryReach> {
  try {
    const { outcome } = await discover.probe(
      target.host,
      target.port,
      target.kind,
      PROBE_TIMEOUT_MS,
    );
    return outcome === 'open' || outcome === 'refused' ? 'online' : 'offline';
  } catch {
    return 'offline';
  }
}

async function probeAll(
  targets: Target[],
  discover: Discover,
  stopped: () => boolean,
): Promise<Map<string, EntryReach>> {
  let next = 0;
  const results = new Map<string, EntryReach>();
  async function worker(): Promise<void> {
    while (!stopped() && next < targets.length) {
      const target = targets[next++];
      const reach = await probeTarget(target, discover);
      if (!stopped()) {
        results.set(target.key, reach);
      }
    }
  }
  await Promise.all(
    Array.from(
      { length: Math.min(MAX_CONCURRENT_PROBES, targets.length) },
      worker,
    ),
  );
  return results;
}

export type ManualReachControl = {
  reach: ManualReach;
  /** Probes now; resolves once the answers are in (pull to refresh). */
  refresh(): Promise<void>;
};

/** Returns manual reachability by entry key; hidden lists retain their last result. */
export function useManualReach(
  entries: ComputerEntry[],
  visible: boolean,
): ManualReach {
  return useManualReachControl(entries, visible).reach;
}

export function useManualReachControl(
  entries: ComputerEntry[],
  visible: boolean,
): ManualReachControl {
  const discover = useCore().discover;
  const [results, setResults] = useState<Record<string, ReachResult>>({});
  const inFlight = useRef<Promise<Map<string, EntryReach>> | null>(null);
  const probeNow = useRef<(() => Promise<void>) | null>(null);

  // Directory refreshes also replace entries. Only changed probe targets restart the timer.
  const targetsJson = JSON.stringify(
    entries
      .flatMap((entry): Target[] => {
        if (entry.kind !== 'manual') {
          return [];
        }
        const kind = entryProtocol(entry);
        return [
          {
            key: entry.key,
            host: entry.manual.host,
            port: entry.manual.port ?? (kind === 'vnc' ? 5900 : 3389),
            kind,
          },
        ];
      })
      .sort((left, right) => left.key.localeCompare(right.key)),
  );
  const targets = useMemo(
    () => JSON.parse(targetsJson) as Target[],
    [targetsJson],
  );

  useEffect(() => {
    setResults(previous => {
      const next: Record<string, ReachResult> = {};
      for (const target of targets) {
        const result = previous[target.key];
        if (
          result !== undefined &&
          result.discover === discover &&
          sameTarget(result.target, target)
        ) {
          next[target.key] = result;
        }
      }
      return next;
    });
    if (!visible || discover === undefined || targets.length === 0) {
      return;
    }

    let stopped = false;
    let running: Promise<void> | null = null;
    const probe = async (): Promise<void> => {
      try {
        // A previous effect's native probes cannot be cancelled; let them drain
        // before starting another pool so visibility/target changes still cap at 8.
        await inFlight.current;
        if (stopped) {
          return;
        }
        const pending = probeAll(targets, discover, () => stopped);
        inFlight.current = pending;
        const probed = await pending;
        if (!stopped) {
          for (const target of targets) {
            const reach = probed.get(target.key);
            if (reach !== undefined) {
              remember(discover, target, reach);
            }
          }
          setResults(previous => {
            const next = { ...previous };
            for (const target of targets) {
              const reach = probed.get(target.key);
              if (reach !== undefined) {
                next[target.key] = { target, discover, reach };
              }
            }
            return next;
          });
        }
        if (inFlight.current === pending) {
          inFlight.current = null;
        }
      } finally {
        running = null;
      }
    };
    // A refresh during a round waits for that round instead of starting one.
    const run = (): Promise<void> => {
      if (stopped) {
        return Promise.resolve();
      }
      running ??= probe();
      return running;
    };

    probeNow.current = run;
    run().catch(() => undefined);
    const timer = setInterval(() => {
      run().catch(() => undefined);
    }, REACH_INTERVAL_MS);
    return () => {
      stopped = true;
      if (probeNow.current === run) {
        probeNow.current = null;
      }
      clearInterval(timer);
    };
  }, [discover, targets, visible]);

  const refresh = useCallback(
    () => probeNow.current?.() ?? Promise.resolve(),
    [],
  );

  const reach = useMemo(
    () =>
      Object.fromEntries(
        targets.map(target => {
          const result = results[target.key];
          const value =
            result !== undefined &&
            result.discover === discover &&
            sameTarget(result.target, target)
              ? result.reach
              : remembered(discover, target) ?? 'unknown';
          return [target.key, value];
        }),
      ),
    [discover, results, targets],
  );

  return useMemo(() => ({ reach, refresh }), [reach, refresh]);
}
