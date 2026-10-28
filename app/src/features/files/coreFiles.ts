import type {
  CoreEvent,
  FileSourceRef,
  FileTransfer,
  TransferId as CoreTransferId,
} from '../../native/rscore/types';
import {
  RsCoreError,
  type RsCoreApi,
  type Unsubscribe,
} from '../../native/rscore/RsCoreApi';
import type {
  FileSource,
  FilesApi,
  Listing,
  Transfer,
  TransferId,
} from './filesApi';
import { friendlyError } from './filesErrors';

type Listener = () => void;

async function forward<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (error) {
    throw friendlyError(error);
  }
}

function toSource(source: FileSourceRef): FileSource {
  return source.kind === 'computer'
    ? { kind: 'computer', hostId: source.hostId }
    : { kind: 'server', serverId: source.serverId };
}

function toTransfer(transfer: FileTransfer): Transfer {
  return {
    transferId: transfer.transferId,
    direction: transfer.direction,
    source: toSource(transfer.source),
    name: transfer.name,
    remotePath: transfer.remotePath,
    localUri:
      transfer.localPath === null
        ? null
        : 'file://' + encodeURI(transfer.localPath),
    size: transfer.size,
    done: transfer.done,
    bytesPerSec: transfer.bytesPerSec,
    state: transfer.state,
    fail: transfer.fail,
    netPath: transfer.netPath,
    savedTo: null,
    startedAt: transfer.startedAt,
    updatedAt: transfer.updatedAt,
  };
}

function pathFromUri(localUri: string): string {
  if (!localUri.startsWith('file://')) {
    throw new RsCoreError('internal', `files.invalidUri: ${localUri}`);
  }
  return decodeURIComponent(localUri.slice('file://'.length));
}

function sortTransfers(transfers: Transfer[]): Transfer[] {
  return [...transfers].sort((a, b) => b.startedAt - a.startedAt);
}

export function createCoreFiles(
  core: RsCoreApi,
): FilesApi & { dispose(): void } {
  let current: Transfer[] | null = null;
  const listeners = new Set<Listener>();
  let disposed = false;
  let unsubscribe: Unsubscribe | null = null;

  const notify = (): void => {
    if (disposed) {
      return;
    }
    for (const listener of [...listeners]) {
      listener();
    }
  };

  const replace = (transfer: FileTransfer): void => {
    current ??= [];
    const next = toTransfer(transfer);
    const index = current.findIndex(
      item => item.transferId === next.transferId,
    );
    if (index === -1) {
      current = sortTransfers([next, ...current]);
    } else {
      const updated = [...current];
      updated[index] = next;
      current = sortTransfers(updated);
    }
    notify();
  };

  const remove = (id: CoreTransferId): void => {
    if (current === null) {
      return;
    }
    const next = current.filter(item => item.transferId !== id);
    if (next.length === current.length) {
      return;
    }
    current = next;
    notify();
  };

  const applyEvent = (event: CoreEvent): void => {
    if (event.type === 'file.transfer') {
      replace(event.transfer);
    } else if (event.type === 'file.transferRemoved') {
      remove(event.transferId);
    }
  };

  try {
    current = sortTransfers(
      core.transfers.list().map(transfer => toTransfer(transfer)),
    );
  } catch (error) {
    current = [];
    if (!(error instanceof RsCoreError && error.kind === 'unsupported')) {
      throw error;
    }
  }
  unsubscribe = core.events.subscribe(applyEvent);

  const files: FilesApi & { dispose(): void } = {
    list: (src: FileSource, path: string | null): Promise<Listing> =>
      forward(async () => {
        const listing =
          src.kind === 'computer'
            ? await core.files.list(src.hostId, path)
            : await core.sftp.list(src.serverId, path);
        return {
          path: listing.path,
          entries: listing.entries.map(entry => ({
            name: entry.name,
            kind: entry.kind,
            size: entry.size,
            mtimeUs: entry.mtimeUs,
            childCount: entry.childCount,
          })),
          netPath: listing.netPath,
        };
      }),
    download: (src: FileSource, remotePath: string): Promise<TransferId> =>
      forward(() =>
        src.kind === 'computer'
          ? core.files.download(src.hostId, remotePath)
          : core.sftp.download(src.serverId, remotePath),
      ),
    upload: (
      src: FileSource,
      localUri: string,
      remoteDir: string | null,
    ): Promise<TransferId> =>
      forward(() =>
        src.kind === 'computer'
          ? core.files.upload(src.hostId, pathFromUri(localUri), remoteDir)
          : core.sftp.upload(src.serverId, pathFromUri(localUri), remoteDir),
      ),
    pause: (id: TransferId): void => {
      try {
        core.transfers.pause(id);
      } catch (error) {
        core.logs.write(
          'warn',
          'files',
          `pause failed: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    },
    resume: (id: TransferId): Promise<void> =>
      forward(() => core.transfers.resume(id)),
    cancel: (id: TransferId): void => {
      try {
        core.transfers.cancel(id);
      } catch (error) {
        core.logs.write(
          'warn',
          'files',
          `cancel failed: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    },
    transfers: (): Transfer[] => current ?? [],
    subscribe: (callback: Listener): Unsubscribe => {
      listeners.add(callback);
      return () => {
        listeners.delete(callback);
      };
    },
    dispose: (): void => {
      disposed = true;
      listeners.clear();
      unsubscribe?.();
      unsubscribe = null;
    },
  };

  return files;
}
