import { useEffect, useState } from 'react';
import type { Transfer } from './filesApi';
import { useFiles } from './FilesContext';

export function useTransfers(): Transfer[] {
  const { files } = useFiles();
  const [transfers, setTransfers] = useState<Transfer[]>(() =>
    files.transfers(),
  );

  useEffect(() => {
    setTransfers(files.transfers());
    return files.subscribe(() => setTransfers(files.transfers()));
  }, [files]);

  return transfers;
}
