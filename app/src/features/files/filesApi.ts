/**
 * 待契约：RsCoreApi 还没有文件 / SFTP 方法（07 只写了
 * `RsCore.sftp.list / download / upload … // M2`），PlatformServices 没有选文件。
 * A16 / P 轨道补齐后用适配层实现这两个接口，页面不改。
 */
import type { Micros, NetPath } from '../../native/rscore/types';
import { RsCoreError, type Unsubscribe } from '../../native/rscore/RsCoreApi';

export type FileSource =
  | { kind: 'computer'; hostId: string } // 文件通道 0x04（F1）
  | { kind: 'server'; serverId: string }; // SFTP（F2），SshServer.serverId

export function sourceKey(source: FileSource): string {
  return source.kind === 'computer'
    ? `computer:${source.hostId}`
    : `server:${source.serverId}`;
}

export type RemoteEntry = {
  name: string;
  kind: 'file' | 'dir' | 'symlink' | 'other';
  size: number | null;
  mtimeUs: Micros | null;
  childCount: number | null;
};

export type Listing = {
  path: string;
  entries: RemoteEntry[];
  netPath: NetPath | null;
};

export type TransferId = string;

// rsp-core 的 7 个状态 + 'paused'（rsp-core 没有暂停态：「待契约，结果里写明」）。
export type TransferState =
  | 'queued'
  | 'running'
  | 'paused'
  | 'interrupted'
  | 'verifying'
  | 'completed'
  | 'failed'
  | 'cancelled';

export type TransferFail =
  | 'io'
  | 'sha256Mismatch'
  | 'resumeMismatch'
  | 'sourceChanged'
  | 'protocol'
  | 'tooLarge'
  | 'notFound'
  | 'permissionDenied'
  | 'notAllowed'
  | 'noSpace'
  | 'authRequired'
  | 'disabled'
  | 'unsupported';

export type Transfer = {
  transferId: TransferId;
  direction: 'upload' | 'download'; // upload = 手机 → 远端；download = 远端 → 手机
  source: FileSource; // 远端那一头
  name: string;
  remotePath: string | null; // download：远端文件完整路径；upload：远端目录，null = 默认目录
  localUri: string | null; // 手机上的文件；download 完成后才有
  size: number;
  done: number;
  bytesPerSec: number;
  state: TransferState;
  fail: TransferFail | null;
  netPath: NetPath | null;
  savedTo: 'photos' | 'files' | null; // download 完成后存到了哪（P 轨道）；null = 只在 App 里
  startedAt: Micros;
  updatedAt: Micros;
};

export interface FilesApi {
  list(src: FileSource, path: string | null): Promise<Listing>; // path null = 主目录
  download(src: FileSource, remotePath: string): Promise<TransferId>;
  upload(
    src: FileSource,
    localUri: string,
    remoteDir: string | null,
  ): Promise<TransferId>;
  pause(id: TransferId): void;
  resume(id: TransferId): Promise<void>; // 暂停、中断、失败后都用它续传
  cancel(id: TransferId): void;
  transfers(): Transfer[];
  subscribe(cb: () => void): Unsubscribe; // 任何一条传输变化后调用
}

export type PickedFile = { uri: string; name: string; size: number | null };

export interface FilePicker {
  pickFiles(): Promise<PickedFile[]>; // 用户取消 → []
  saveToFiles(uri: string, name: string): Promise<boolean>; // 存到「文件」App（P 轨道）
}

function unsupported(): RsCoreError {
  return new RsCoreError('unsupported', 'files: 待契约');
}

export const UNSUPPORTED_FILES: FilesApi = {
  list: async () => {
    throw unsupported();
  },
  download: async () => {
    throw unsupported();
  },
  upload: async () => {
    throw unsupported();
  },
  pause: () => {},
  resume: async () => {
    throw unsupported();
  },
  cancel: () => {},
  transfers: () => [],
  subscribe: () => () => {},
};

export const UNSUPPORTED_PICKER: FilePicker = {
  pickFiles: async () => {
    throw unsupported();
  },
  saveToFiles: async () => {
    throw unsupported();
  },
};

export function isUnsupported(error: unknown): boolean {
  return error instanceof RsCoreError && error.kind === 'unsupported';
}
