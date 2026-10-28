import type { Computer, OtherDevice } from '../../native/rscore/types';
import type { ManualComputer } from '../servers/manual';

export type ComputerEntry =
  /** 装了 Host（目录 computers）。 */
  | { kind: 'host'; key: string; computer: Computer }
  /** 没装 Host、在 tailnet 里（目录 others）。 */
  | { kind: 'tailnet'; key: string; device: OtherDevice }
  /** 没装 Host、用户手动添加或从局域网发现后保存的。 */
  | {
      kind: 'manual';
      key: string;
      manual: ManualComputer & { os: 'macos' | 'windows' };
    };

export type EntryReach = 'online' | 'offline' | 'unknown';
export type ConnectVia = 'auto' | 'native' | 'host';
