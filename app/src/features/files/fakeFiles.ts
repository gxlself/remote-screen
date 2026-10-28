import { FIXTURE_NOW, sshServers } from '../../native/rscore/fixtures';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import {
  sourceKey,
  type FilePicker,
  type FileSource,
  type FilesApi,
  type Listing,
  type PickedFile,
  type Transfer,
  type TransferId,
} from './filesApi';

type Call = { method: string; args: unknown[] };
type Listener = () => void;

function copy<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map(item => copy(item)) as T;
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, copy(item)]),
    ) as T;
  }
  return value;
}

const MIN = 60e6;
const DAY = 86400e6;

const serverId = (name: string): string => {
  const server = sshServers.find(item => item.name === name);
  if (server === undefined) {
    throw new Error(`missing SSH server fixture: ${name}`);
  }
  return server.serverId;
};

export const srcStudio: FileSource = {
  kind: 'computer',
  hostId: 'h7v2m9q3k8x4tb1a',
};
export const srcOffice: FileSource = {
  kind: 'computer',
  hostId: 'w3k8p2x7m4q9tb6c',
};
export const srcGpu: FileSource = {
  kind: 'server',
  serverId: serverId('gpu-train-02'),
};
export const srcNas: FileSource = {
  kind: 'server',
  serverId: serverId('nas-home'),
};

const homeListing: Listing = {
  path: '/Users/alice',
  netPath: 'tailnetDirect',
  entries: [
    {
      name: '桌面',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - 37 * MIN,
      childCount: 5,
    },
    {
      name: '文稿',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - 2 * DAY,
      childCount: 21,
    },
    {
      name: '下载',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - DAY,
      childCount: 64,
    },
  ],
};

const desktopListing: Listing = {
  path: '/Users/alice/桌面',
  netPath: 'tailnetDirect',
  entries: [
    {
      name: 'remote-screen',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - 30 * MIN,
      childCount: 38,
    },
    {
      name: '设计稿',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - DAY,
      childCount: 12,
    },
    {
      name: '截屏 10.20.png',
      kind: 'file',
      size: 2516582,
      mtimeUs: FIXTURE_NOW - 37 * MIN,
      childCount: null,
    },
    {
      name: 'build-0.3.1.dmg',
      kind: 'file',
      size: 196712858,
      mtimeUs: FIXTURE_NOW - 66 * MIN,
      childCount: null,
    },
    {
      name: 'notes-1020.md',
      kind: 'file',
      size: 6144,
      mtimeUs: FIXTURE_NOW - 88 * MIN,
      childCount: null,
    },
  ],
};

const gpuListing: Listing = {
  path: '/home/alice',
  netPath: 'tailnetDirect',
  entries: [
    {
      name: 'checkpoints',
      kind: 'dir',
      size: null,
      mtimeUs: FIXTURE_NOW - DAY,
      childCount: 8,
    },
    {
      name: 'train-1020.log',
      kind: 'file',
      size: 4299162,
      mtimeUs: FIXTURE_NOW - 46 * MIN,
      childCount: null,
    },
  ],
};

export const listingFixtures: Record<string, Listing> = {
  [`${sourceKey(srcStudio)}|`]: homeListing,
  [`${sourceKey(srcStudio)}|/Users/alice`]: homeListing,
  [`${sourceKey(srcStudio)}|/Users/alice/桌面`]: desktopListing,
  [`${sourceKey(srcGpu)}|`]: gpuListing,
  [`${sourceKey(srcGpu)}|/home/alice`]: gpuListing,
};

export const transferFixtures: Transfer[] = [
  {
    transferId: 't-img',
    direction: 'upload',
    source: srcOffice,
    name: 'IMG_4021.HEIC',
    remotePath: null,
    localUri: null,
    size: 29779558,
    done: 19084083,
    bytesPerSec: 5033165,
    state: 'running',
    fail: null,
    netPath: 'tailnetDerp',
    savedTo: null,
    startedAt: FIXTURE_NOW - 10e6,
    updatedAt: FIXTURE_NOW,
  },
  {
    transferId: 't-dmg',
    direction: 'download',
    source: srcStudio,
    name: 'build-0.3.1.dmg',
    remotePath: '/Users/alice/桌面/build-0.3.1.dmg',
    localUri: null,
    size: 196712858,
    done: 43306189,
    bytesPerSec: 90177536,
    state: 'running',
    fail: null,
    netPath: 'tailnetDirect',
    savedTo: null,
    startedAt: FIXTURE_NOW - 60e6,
    updatedAt: FIXTURE_NOW,
  },
  {
    transferId: 't-tar',
    direction: 'download',
    source: srcNas,
    name: 'dataset-1019.tar',
    remotePath: '/home/alice/dataset-1019.tar',
    localUri: null,
    size: 4509715661,
    done: 2841120870,
    bytesPerSec: 0,
    state: 'interrupted',
    fail: null,
    netPath: 'tailnetDerp',
    savedTo: null,
    startedAt: FIXTURE_NOW - 5 * MIN,
    updatedAt: FIXTURE_NOW - 5 * MIN,
  },
  {
    transferId: 't-png',
    direction: 'download',
    source: srcStudio,
    name: '截屏 10.20.png',
    remotePath: '/Users/alice/桌面/截屏 10.20.png',
    localUri: 'file:///fake/截屏 10.20.png',
    size: 2516582,
    done: 2516582,
    bytesPerSec: 0,
    state: 'completed',
    fail: null,
    netPath: 'tailnetDirect',
    savedTo: 'photos',
    startedAt: FIXTURE_NOW - 37 * MIN,
    updatedAt: FIXTURE_NOW - 37 * MIN,
  },
  {
    transferId: 't-log',
    direction: 'download',
    source: srcGpu,
    name: 'train-1020.log',
    remotePath: '/home/alice/train-1020.log',
    localUri: 'file:///fake/train-1020.log',
    size: 4299162,
    done: 4299162,
    bytesPerSec: 0,
    state: 'completed',
    fail: null,
    netPath: 'tailnetDirect',
    savedTo: 'files',
    startedAt: FIXTURE_NOW - 46 * MIN,
    updatedAt: FIXTURE_NOW - 46 * MIN,
  },
  {
    transferId: 't-xlsx',
    direction: 'upload',
    source: srcOffice,
    name: '季度报表-Q3.xlsx',
    remotePath: null,
    localUri: null,
    size: 880640,
    done: 880640,
    bytesPerSec: 0,
    state: 'completed',
    fail: null,
    netPath: 'intranet',
    savedTo: null,
    startedAt: FIXTURE_NOW - 71 * MIN,
    updatedAt: FIXTURE_NOW - 71 * MIN,
  },
];

function lastSegment(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1] ?? '';
}

export class FakeFiles implements FilesApi, FilePicker {
  readonly calls: Call[] = [];
  picked: PickedFile[];

  private listings: Record<string, Listing>;
  private transferList: Transfer[];
  private readonly failures = new Map<string, Error>();
  private readonly listeners = new Set<Listener>();
  private downloadSequence = 0;
  private uploadSequence = 0;

  constructor(opts?: {
    listings?: Record<string, Listing>;
    transfers?: Transfer[];
    picked?: PickedFile[];
  }) {
    this.listings = copy(opts?.listings ?? listingFixtures);
    this.transferList = copy(opts?.transfers ?? transferFixtures);
    this.picked = copy(opts?.picked ?? []);
  }

  failNext(method: string, error: Error): void {
    this.failures.set(method, error);
  }

  emit(transfer: Transfer): void {
    const index = this.transferList.findIndex(
      item => item.transferId === transfer.transferId,
    );
    const next = copy(transfer);
    if (index === -1) {
      this.transferList.push(next);
    } else {
      this.transferList[index] = next;
    }
    this.notify();
  }

  subscriberCount(): number {
    return this.listeners.size;
  }

  list = async (src: FileSource, path: string | null): Promise<Listing> => {
    this.record('list', src, path);
    const key = `${sourceKey(src)}|${path ?? ''}`;
    const listing = this.listings[key];
    if (listing !== undefined) {
      return copy(listing);
    }
    const prefix = `${sourceKey(src)}|`;
    if (!Object.keys(this.listings).some(item => item.startsWith(prefix))) {
      throw new RsCoreError('unsupported', 'browse not supported');
    }
    throw new RsCoreError('http', 'not found', 404, 'not_found');
  };

  download = async (
    src: FileSource,
    remotePath: string,
  ): Promise<TransferId> => {
    this.record('download', src, remotePath);
    this.downloadSequence += 1;
    const transferId = `t-dl-${this.downloadSequence}`;
    this.transferList.push({
      transferId,
      direction: 'download',
      source: copy(src),
      name: lastSegment(remotePath),
      remotePath,
      localUri: null,
      size: 0,
      done: 0,
      bytesPerSec: 0,
      state: 'queued',
      fail: null,
      netPath: null,
      savedTo: null,
      startedAt: FIXTURE_NOW,
      updatedAt: FIXTURE_NOW,
    });
    this.notify();
    return transferId;
  };

  upload = async (
    src: FileSource,
    localUri: string,
    remoteDir: string | null,
  ): Promise<TransferId> => {
    this.record('upload', src, localUri, remoteDir);
    this.uploadSequence += 1;
    const transferId = `t-up-${this.uploadSequence}`;
    this.transferList.push({
      transferId,
      direction: 'upload',
      source: copy(src),
      name: decodeURIComponent(lastSegment(localUri)),
      remotePath: remoteDir,
      localUri: null,
      size: 0,
      done: 0,
      bytesPerSec: 0,
      state: 'queued',
      fail: null,
      netPath: null,
      savedTo: null,
      startedAt: FIXTURE_NOW,
      updatedAt: FIXTURE_NOW,
    });
    this.notify();
    return transferId;
  };

  pause = (id: TransferId): void => {
    this.record('pause', id);
    const transfer = this.find(id);
    if (
      transfer === undefined ||
      (transfer.state !== 'running' && transfer.state !== 'queued')
    ) {
      return;
    }
    transfer.state = 'paused';
    transfer.bytesPerSec = 0;
    this.notify();
  };

  resume = async (id: TransferId): Promise<void> => {
    this.record('resume', id);
    const transfer = this.find(id);
    if (
      transfer === undefined ||
      (transfer.state !== 'paused' &&
        transfer.state !== 'interrupted' &&
        transfer.state !== 'failed')
    ) {
      return;
    }
    transfer.state = 'running';
    transfer.fail = null;
    this.notify();
  };

  cancel = (id: TransferId): void => {
    this.record('cancel', id);
    const transfer = this.find(id);
    if (transfer === undefined) {
      return;
    }
    transfer.state = 'cancelled';
    this.notify();
  };

  transfers = (): Transfer[] => this.transferList.slice();

  subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  pickFiles = async (): Promise<PickedFile[]> => {
    this.record('pickFiles');
    return copy(this.picked);
  };

  saveToFiles = async (uri: string, name: string): Promise<boolean> => {
    this.record('saveToFiles', uri, name);
    return true;
  };

  private record(method: string, ...args: unknown[]): void {
    this.calls.push({ method, args });
    const failure = this.failures.get(method);
    if (failure !== undefined) {
      this.failures.delete(method);
      throw failure;
    }
  }

  private find(id: TransferId): Transfer | undefined {
    return this.transferList.find(item => item.transferId === id);
  }

  private notify(): void {
    for (const listener of [...this.listeners]) {
      listener();
    }
  }
}

export function createFakeFiles(opts?: {
  listings?: Record<string, Listing>;
  transfers?: Transfer[];
  picked?: PickedFile[];
}): FakeFiles {
  return new FakeFiles(opts);
}
