import { useMemo } from 'react';
import type { Directory } from '../../native/rscore/types';
import { useDirectory } from '../data/queries';
import { useManualServers } from '../servers/useManualServers';
import { buildEntries } from './entries';
import type { ComputerEntry } from './types';

const EMPTY_DIRECTORY: Directory = {
  generatedAt: 0,
  computers: [],
  servers: [],
  others: [],
};

export function useComputerEntries(): ComputerEntry[] {
  const { directory } = useDirectory();
  const { computers } = useManualServers();

  return useMemo(
    () => buildEntries(directory ?? EMPTY_DIRECTORY, computers),
    [directory, computers],
  );
}
