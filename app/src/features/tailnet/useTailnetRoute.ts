import type { TailnetRouteStatus } from '../../native/rscore/types';
import { useNetwork } from '../network/store';

export function useTailnetRoute(): TailnetRouteStatus | null {
  return useNetwork(state => state.status?.tailnet ?? null);
}
