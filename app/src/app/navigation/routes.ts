import type {
  CompositeScreenProps,
  NavigatorScreenParams,
} from '@react-navigation/native';
import type { BottomTabScreenProps } from '@react-navigation/bottom-tabs';
import type { NativeStackScreenProps } from '@react-navigation/native-stack';
import type { AdapterContext, AdapterTarget, Address } from '../../native/rscore/types';

export type MainTabParamList = {
  Home: undefined;
  Sessions: undefined;
  Files: undefined;
  Settings: undefined;
};

export type RootStackParamList = {
  OnboardTailnet: undefined;
  OnboardDiscover: undefined;
  OnboardProtect: undefined;
  MainTabs: NavigatorScreenParams<MainTabParamList> | undefined;
  DeviceSheet: { hostId: string };
  /** V10：没装 Host 的电脑（tailnet / 手动）的设备卡片；entryKey 即 ComputerEntry.key。 */
  NativeSheet: { entryKey: string };
  DeviceRoutes: { hostId: string };
  PairAdd: { tab?: 'scan' | 'code'; hostId?: string; addr?: string; port?: number } | undefined;
  PairWaiting:
    | { qrText: string }
    | { addr: string; port?: number; code: string }
    | { fileId: string };
  // 导入配对文件。
  PairFile: { text?: string; uri?: string } | undefined;
  PairDone: {
    hostId: string;
    hostName: string;
    permission?: 'viewOnly' | 'control' | 'unattended';
    path?: 'tailnetDirect' | 'tailnetDerp' | 'intranet' | 'lan' | 'manual';
    latencyMs?: number;
  };
  Session: {
    hostId: string;
    sessionId?: string;
    viewOnly?: boolean;
    displayId?: number;
    /** C7：系统共享没连上、改走 Host 时，会话页顶部提示一次的文案。 */
    notice?: string;
    /** 原地换成另一台时递增，页面按它重新挂载。 */
    switchSeq?: number;
  };
  SessionEnded: {
    hostId: string;
    hostName?: string;
    sessionId?: string;
    code?: number;
    reason?: string;
    detail?: Record<string, string>;
  };
  AdapterSession: {
    target: AdapterTarget;
    name?: string;
    addrKind?: Address['kind'] | 'manual';
    context?: AdapterContext;
    /** V33：接回已收起的会话，不重新连接。 */
    sessionId?: string;
    /** 原地换成另一台时递增，页面按它重新挂载。 */
    switchSeq?: number;
  };
  /** V10：添加没装 Host 的电脑（局域网发现 + 手动地址）。 */
  AddComputer: undefined;
  /** V10：教用户在电脑上打开屏幕共享 / 远程桌面。 */
  EnableSharing: {
    os: 'macos' | 'windows';
    name?: string;
    /** 打开后点「重试」时重新走 C7 路由用。 */
    entryKey?: string;
  };
  // Error 120 can prefill the command for installation through SSH.
  Terminal: { serverId: string; sshId?: string; prefill?: string };
  RootConfirm: { sshId: string; serverId: string; url: string; user: string };
  HostKeyPrompt: {
    sshId: string;
    serverId: string;
    fp: string;
    keyType: string;
    kind: 'first' | 'changed';
    oldFp?: string;
  };
  SshKeys: undefined;
  Recordings: undefined;
  RecordingPlayer: { recordingId: string };
  AppLock: undefined;
  Diagnostics: { hostId?: string } | undefined;
  Networks: undefined;
  AddByAddress: undefined;
  /**
   * prefill：首页的 Tailscale 建议行或按地址添加带来的名称和地址，只在新建时用；
   * connectAfterSave 只有按地址添加会带，保存后直接打开终端。
   */
  AddServer:
    | {
        serverId?: string;
        prefill?: {
          name: string;
          host: string;
          port?: number;
          connectAfterSave?: boolean;
        };
      }
    | undefined;
  DevSelfTest: undefined;
};

export type RootScreenProps<R extends keyof RootStackParamList> =
  NativeStackScreenProps<RootStackParamList, R>;
export type TabScreenProps<R extends keyof MainTabParamList> =
  CompositeScreenProps<
    BottomTabScreenProps<MainTabParamList, R>,
    NativeStackScreenProps<RootStackParamList>
  >;
