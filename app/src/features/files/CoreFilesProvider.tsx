import { useEffect, useMemo } from 'react';
import type { ReactNode } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import NativeRsFiles from '../../native/specs/NativeRsFiles';
import { createCoreFiles } from './coreFiles';
import { FilesProvider } from './FilesContext';
import { createNativePicker } from './nativePicker';
import type { Transfer } from './filesApi';

function hasActiveTransfer(transfers: Transfer[]): boolean {
  return transfers.some(
    transfer =>
      transfer.state === 'queued' ||
      transfer.state === 'running' ||
      transfer.state === 'verifying',
  );
}

export function CoreFilesProvider({ children }: { children: ReactNode }) {
  const core = useCore();
  const files = useMemo(() => createCoreFiles(core), [core]);
  const picker = useMemo(() => createNativePicker(), []);

  useEffect(() => () => files.dispose(), [files]);

  useEffect(() => {
    let active = false;
    const update = () => {
      const next = hasActiveTransfer(files.transfers());
      if (next === active) {
        return;
      }
      active = next;
      NativeRsFiles?.setTransfersActive(next);
    };
    const unsubscribe = files.subscribe(update);
    update();
    return unsubscribe;
  }, [files]);

  return (
    <FilesProvider files={files} picker={picker}>
      {children}
    </FilesProvider>
  );
}

export default CoreFilesProvider;
