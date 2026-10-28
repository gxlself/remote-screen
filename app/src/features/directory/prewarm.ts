import { useEffect, useRef } from 'react';
import { AppState } from 'react-native';
import { useCore } from '../../native/rscore/CoreContext';
import { useSetting } from '../settings/store';
import type { CardModel } from './cardModel';

const DEFAULT_PREWARM_LIMIT = 4;

export function pickPrewarm(
  cards: CardModel[],
  limit = DEFAULT_PREWARM_LIMIT,
): string[] {
  return cards
    .filter(card => card.group <= 1 && card.tap.kind === 'open')
    .slice(0, limit)
    .map(card => card.hostId);
}

function signature(hostIds: string[]): string {
  return [...hostIds].sort().join('\n');
}

export function usePrewarm(hostIds: string[]): void {
  const core = useCore();
  const [enabled] = useSetting('prewarm.enabled');
  const hostIdsRef = useRef(hostIds);
  const enabledRef = useRef(enabled);
  const state = AppState.currentState;
  const activeRef = useRef(state !== 'inactive' && state !== 'background');
  const lastSignatureRef = useRef<string | null>(null);

  hostIdsRef.current = hostIds;
  enabledRef.current = enabled;

  useEffect(() => {
    if (!enabled) {
      lastSignatureRef.current = null;
      return;
    }
    if (!activeRef.current || hostIds.length === 0) {
      return;
    }
    const nextSignature = signature(hostIds);
    if (lastSignatureRef.current === nextSignature) {
      return;
    }
    lastSignatureRef.current = nextSignature;
    core.sessions.prewarm([...hostIds]);
  }, [core, enabled, hostIds]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', nextState => {
      const wasActive = activeRef.current;
      activeRef.current = nextState === 'active';
      if (!activeRef.current) {
        return;
      }
      if (wasActive || !enabledRef.current || hostIdsRef.current.length === 0) {
        return;
      }
      lastSignatureRef.current = signature(hostIdsRef.current);
      core.sessions.prewarm([...hostIdsRef.current]);
    });
    return () => subscription.remove();
  }, [core]);
}
