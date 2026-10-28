import { useCallback, useEffect, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type {
  RootScreenProps,
  RootStackParamList,
} from '../../app/navigation/routes';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useDirectory } from '../../features/data/queries';
import { qk } from '../../features/data/queryClient';
import {
  initialPairState,
  pairReducer,
  pairSteps,
} from '../../features/pairing/machine';
import type { PairMethod } from '../../features/pairing/outcome';
import { recallPairFile } from '../../features/pairing/pairFileCache';
import { pairFile } from '../../i18n/zh/pairFile';
import { pairWaiting } from '../../i18n/zh/pairWaiting';
import { useCore } from '../../native/rscore/CoreContext';
import type {
  Computer,
  PairOutcome,
  PairTarget,
} from '../../native/rscore/types';

type PairingParams = RootStackParamList['PairWaiting'];
type PairingNavigation = RootScreenProps<'PairWaiting'>['navigation'];
type PairingTarget =
  | PairTarget
  | { addr: string; port?: number; code: string }
  | { fileId: string };

export type PairAddress = { addr: string; port?: number };

/** Rust reports an address that does not answer as an RSP Host this way. */
const NOT_HOST_MESSAGE = 'not a remote host';

export function isNotHostOutcome(outcome: PairOutcome): boolean {
  return (
    outcome.status === 'error' &&
    outcome.message.toLowerCase().includes(NOT_HOST_MESSAGE)
  );
}

export type PairHost = {
  hostId: string | null;
  name: string;
  node: string | null;
  address: string | null;
  os: Computer['os'] | null;
  hostVersion: string | null;
};

export type PairingModel = {
  state: ReturnType<typeof pairReducer>;
  steps: ReturnType<typeof pairSteps>;
  host: PairHost;
  method: PairMethod;
  /** Set when pairing by a typed address instead of a known host id. */
  address: PairAddress | null;
  cancel: () => void;
  restart: () => void;
};

type ParsedParams = {
  target: PairingTarget | null;
  hostId: string | null;
  initialName: string | null;
  initialAddresses: string[];
  invalid: boolean;
};

function parseParams(
  params: PairingParams,
  core: ReturnType<typeof useCore>,
): ParsedParams {
  if ('qrText' in params) {
    const target = core.pairing.parseQr(params.qrText);
    if (target === null) {
      return {
        target: null,
        hostId: null,
        initialName: null,
        initialAddresses: [],
        invalid: true,
      };
    }
    return {
      target,
      hostId: target.hostId,
      initialName: target.name.trim() === '' ? null : target.name,
      initialAddresses: target.addrs,
      invalid: false,
    };
  }

  if ('fileId' in params) {
    const summary = recallPairFile(params.fileId);
    return {
      target: { fileId: params.fileId },
      hostId: summary?.hostId ?? null,
      initialName: summary?.hostName ?? null,
      initialAddresses: summary?.addrs ?? [],
      invalid: false,
    };
  }

  return {
    target: { addr: params.addr, port: params.port, code: params.code },
    hostId: null,
    initialName: null,
    initialAddresses: [params.addr],
    invalid: false,
  };
}

function initialState(parsed: ParsedParams) {
  if (!parsed.invalid) {
    return initialPairState;
  }
  return pairReducer(initialPairState, {
    type: 'local.outcome',
    outcome: { status: 'error', message: pairWaiting.invalidQr },
  });
}

function errorOutcome(error: unknown): PairOutcome {
  return {
    status: 'error',
    message: error instanceof Error ? error.message : '',
  };
}

function addressOf(
  computer: Computer | undefined,
  fallback: string[],
): string | null {
  const address =
    computer?.addresses.find(item => item.kind === 'tailnet') ??
    computer?.addresses[0];
  return address?.addr ?? fallback[0] ?? null;
}

export function usePairing(
  params: PairingParams,
  navigation: PairingNavigation,
): PairingModel {
  const core = useCore();
  const queryClient = useQueryClient();
  const { directory } = useDirectory();
  const [parsed] = useState(() => parseParams(params, core));
  const [state, setState] = useState(() => initialState(parsed));
  const mountedRef = useRef(false);
  const activeRef = useRef(false);
  const attemptRef = useRef(0);
  const leftRef = useRef(false);
  const navigatedRef = useRef(false);

  const computer = directory?.computers.find(
    item => item.hostId === parsed.hostId,
  );
  const host: PairHost = {
    hostId: parsed.hostId,
    name:
      parsed.initialName ??
      computer?.name ??
      ('fileId' in params ? pairFile.unknownHost : pairWaiting.unknownHost),
    node: computer?.node ?? null,
    address:
      'fileId' in params
        ? parsed.initialAddresses[0] ?? null
        : addressOf(computer, parsed.initialAddresses),
    os: computer?.os ?? null,
    hostVersion: computer?.hostVersion ?? null,
  };
  const method: PairMethod =
    'fileId' in params ? 'file' : 'qrText' in params ? 'qr' : 'code';
  const address: PairAddress | null =
    'addr' in params ? { addr: params.addr, port: params.port } : null;

  const runStart = useCallback(
    (target: PairingTarget) => {
      const attempt = attemptRef.current + 1;
      attemptRef.current = attempt;
      activeRef.current = true;
      navigatedRef.current = false;
      setState(initialPairState);

      let pending: Promise<PairOutcome>;
      try {
        pending = core.pairing.start(target);
      } catch (error) {
        activeRef.current = false;
        setState(current =>
          pairReducer(current, {
            type: 'local.outcome',
            outcome: errorOutcome(error),
          }),
        );
        return;
      }

      Promise.resolve(pending).then(
        outcome => {
          if (attempt !== attemptRef.current || !mountedRef.current) {
            return;
          }
          activeRef.current = false;
          setState(current =>
            pairReducer(current, { type: 'local.outcome', outcome }),
          );
        },
        (error: unknown) => {
          if (attempt !== attemptRef.current || !mountedRef.current) {
            return;
          }
          activeRef.current = false;
          setState(current =>
            pairReducer(current, {
              type: 'local.outcome',
              outcome: errorOutcome(error),
            }),
          );
        },
      );
    },
    [core],
  );

  const cancel = useCallback(() => {
    if (leftRef.current) {
      return;
    }
    leftRef.current = true;
    activeRef.current = false;
    attemptRef.current += 1;
    core.pairing.cancel();
    navigation.goBack();
  }, [core, navigation]);

  const restart = useCallback(() => {
    if (parsed.target === null) {
      setState(initialState(parsed));
      return;
    }
    runStart(parsed.target);
  }, [parsed, runStart]);

  useCoreEvent('pairing.progress', event => {
    if (mountedRef.current) {
      setState(current => pairReducer(current, event));
    }
  });

  useCoreEvent('pairing.done', event => {
    if (mountedRef.current) {
      activeRef.current = false;
      setState(current => pairReducer(current, event));
    }
  });

  useEffect(() => {
    mountedRef.current = true;
    if (parsed.target !== null) {
      runStart(parsed.target);
    }
    return () => {
      mountedRef.current = false;
      if (activeRef.current) {
        activeRef.current = false;
        core.pairing.cancel();
      }
    };
  }, [core, parsed, runStart]);

  useEffect(() => {
    const outcome = state.outcome;
    if (
      state.phase !== 'success' ||
      outcome === null ||
      outcome.status !== 'ok' ||
      navigatedRef.current
    ) {
      return;
    }
    navigatedRef.current = true;
    const hostId = outcome.hostId;
    const doneParams: RootStackParamList['PairDone'] = {
      hostId,
      hostName: outcome.hostName || host.name,
    };
    if (outcome.permission !== undefined) {
      doneParams.permission = outcome.permission;
    }
    if (outcome.path !== undefined) {
      doneParams.path = outcome.path;
    }
    if (outcome.latencyMs !== undefined) {
      doneParams.latencyMs = outcome.latencyMs;
    }

    // Paired already: show it now. The directory refresh may be slow and
    // PairDone fills in the addresses when it lands.
    navigation.replace('PairDone', doneParams);
    Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.directory }),
      queryClient.invalidateQueries({ queryKey: qk.pairedHosts }),
    ]).catch(() => undefined);
  }, [
    host.name,
    method,
    navigation,
    parsed.hostId,
    queryClient,
    state.outcome,
    state.phase,
  ]);

  return {
    state,
    steps: pairSteps(state),
    host,
    method,
    address,
    cancel,
    restart,
  };
}
