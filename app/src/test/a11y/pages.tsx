import type { ComponentType, ReactElement } from 'react';
import { act, cleanup, fireEvent } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type {
  RootScreenProps,
  RootStackParamList,
  TabScreenProps,
} from '../../app/navigation/routes';
import { resetSshStore } from '../../features/sessions/sshStore';
import type { LocalDirectorySeed } from '../localDirectory';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  computers,
  FIXTURE_NOW,
  liveSession,
  pairQrStudio,
  sshServers,
} from '../../native/rscore/fixtures';
import Home from '../../screens/Home/index';
import DeviceSheet from '../../screens/DeviceSheet/index';
import DeviceRoutes from '../../screens/DeviceRoutes/index';
import PairAdd from '../../screens/PairAdd/index';
import PairWaiting from '../../screens/PairWaiting/index';
import Session from '../../screens/Session/index';
import SessionEnded from '../../screens/SessionEnded/index';
import Sessions from '../../screens/Sessions/index';
import Settings from '../../screens/Settings/index';
import SshKeys from '../../screens/SshKeys/index';
import AddServer from '../../screens/AddServer/index';
import Terminal from '../../screens/Terminal/index';
import Diagnostics from '../../screens/Diagnostics/index';
import AppLock from '../../screens/AppLock/index';
import Networks from '../../screens/Networks/index';
import AddByAddress from '../../screens/AddByAddress/index';
import OnboardTailnet from '../../screens/OnboardTailnet/index';
import OnboardDiscover from '../../screens/OnboardDiscover/index';
import OnboardProtect from '../../screens/OnboardProtect/index';
import { mockPhoneDimensions, mockWindowDimensions } from '../dimensions';
import { mockNavigation, mockRoute } from '../navigation';
import { renderWithProviders } from '../render';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { FilesProvider } from '../../features/files/FilesContext';
import { createFakeFiles } from '../../features/files/fakeFiles';
import { pairFileText } from '../../native/rscore/fixtures';
import PairDone from '../../screens/PairDone/index';
import PairFile from '../../screens/PairFile/index';
import Files from '../../screens/Files/index';
import HostKeyPrompt from '../../screens/HostKeyPrompt/index';
import RootConfirm from '../../screens/RootConfirm/index';
import AppLockGate from '../../screens/AppLockGate/index';
import ConnectingOverlay from '../../screens/Session/ConnectingOverlay';
import ApprovalOverlay from '../../screens/Session/ApprovalOverlay';
import ReconnectOverlay from '../../screens/Session/ReconnectOverlay';
import IdleOverlay from '../../screens/Session/IdleOverlay';
import SharedOverlay from '../../screens/Session/SharedOverlay';
import LocalUseBar from '../../screens/Session/LocalUseBar';
import type { SessionCtx } from '../../screens/Session/slots';
import SessionToolbar from '../../screens/Session/SessionToolbar';

/** Session participants' account; local data only, no Hub profile. */
const SESSION_USER = 'alice@';
const studio = computers[0];
const office = computers[1];
export const clients: QueryClient[] = [];
export type ProvidersOptions = NonNullable<
  Parameters<typeof renderWithProviders>[1]
>;
export type Page = {
  name: string;
  ui: () => ReactElement;
  ready: string | RegExp;
  wide?: boolean;
  options?: () => ProvidersOptions;
};

export function rootPage<R extends keyof RootStackParamList>(
  Page: ComponentType<RootScreenProps<R>>,
  name: R,
  params?: RootStackParamList[R],
) {
  return (
    <Page navigation={mockNavigation<R>()} route={mockRoute(name, params)} />
  );
}

export function tabPage<R extends 'Home' | 'Sessions' | 'Settings' | 'Files'>(
  Page: ComponentType<TabScreenProps<R>>,
  name: R,
) {
  return (
    <Page
      navigation={
        mockNavigation<'MainTabs'>() as unknown as TabScreenProps<R>['navigation']
      }
      route={
        {
          key: `${name}-a11y`,
          name,
          params: undefined,
        } as unknown as TabScreenProps<R>['route']
      }
    />
  );
}

/** The paired Hosts, their reach and the tailnet naming their nodes. */
export const LOCAL_DIRECTORY: LocalDirectorySeed = { peers: [] };

export async function mount(ui: ReactElement, options: ProvidersOptions = {}) {
  const view = await renderWithProviders(ui, {
    localDirectory: LOCAL_DIRECTORY,
    ...options,
    // The audit covers every page, the Tailscale ones too: 外网访问 on.
    settings: {
      'session.coachShown': true,
      'tailnet.enabled': true,
      ...options.settings,
    },
  });
  clients.push(view.queryClient);
  await act(() => jest.advanceTimersByTimeAsync(1));
  return view;
}

export const pages: Page[] = [
  { name: 'Home narrow', ui: () => tabPage(Home, 'Home'), ready: studio.name },
  {
    name: 'Home wide',
    ui: () => tabPage(Home, 'Home'),
    ready: studio.name,
    wide: true,
  },
  {
    name: 'DeviceSheet',
    ui: () => rootPage(DeviceSheet, 'DeviceSheet', { hostId: studio.hostId }),
    ready: studio.name,
  },
  {
    name: 'DeviceRoutes',
    ui: () => rootPage(DeviceRoutes, 'DeviceRoutes', { hostId: studio.hostId }),
    ready: studio.name,
  },
  {
    name: 'PairAdd scan',
    ui: () => rootPage(PairAdd, 'PairAdd'),
    ready: '配对电脑',
  },
  {
    name: 'PairAdd code',
    ui: () => rootPage(PairAdd, 'PairAdd', { tab: 'code' }),
    ready: '配对电脑',
  },
  {
    name: 'PairWaiting',
    ui: () => rootPage(PairWaiting, 'PairWaiting', { qrText: pairQrStudio }),
    ready: '正在和 Studio Mac mini 配对',
  },
  {
    name: 'Sessions',
    ui: () => tabPage(Sessions, 'Sessions'),
    ready: studio.name,
  },
  {
    name: 'Settings',
    ui: () => tabPage(Settings, 'Settings'),
    ready: '已配对的电脑',
  },
  {
    name: 'SshKeys',
    ui: () => rootPage(SshKeys, 'SshKeys'),
    ready: '通过已有会话安装公钥',
  },
  {
    name: 'AddServer',
    ui: () => rootPage(AddServer, 'AddServer'),
    ready: '手动添加服务器',
  },
  {
    name: 'Terminal',
    ui: () =>
      rootPage(Terminal, 'Terminal', { serverId: sshServers[0].serverId }),
    ready: sshServers[0].name,
  },
  {
    name: 'Diagnostics',
    ui: () => rootPage(Diagnostics, 'Diagnostics', { hostId: studio.hostId }),
    ready: studio.name,
  },
  {
    name: 'AppLock',
    ui: () => rootPage(AppLock, 'AppLock'),
    ready: '打开 App 时验证',
  },
  {
    name: 'Networks',
    ui: () => rootPage(Networks, 'Networks'),
    ready: '还没有登记网络',
  },
  {
    name: 'AddByAddress',
    ui: () => rootPage(AddByAddress, 'AddByAddress'),
    ready: '按地址添加',
  },
  {
    name: 'OnboardTailnet',
    ui: () => rootPage(OnboardTailnet, 'OnboardTailnet'),
    ready: '连接到你的 tailnet',
  },
  {
    name: 'OnboardDiscover',
    ui: () => rootPage(OnboardDiscover, 'OnboardDiscover'),
    ready: '找到了 4 台电脑',
  },
  {
    name: 'OnboardProtect',
    ui: () => rootPage(OnboardProtect, 'OnboardProtect'),
    ready: 'Face ID 解锁',
  },
  {
    name: 'PairDone',
    ui: () =>
      rootPage(PairDone, 'PairDone', {
        hostId: studio.hostId,
        hostName: studio.name,
        permission: 'unattended',
        path: 'tailnetDirect',
        latencyMs: 6,
      }),
    ready: 'Studio Mac mini 已配对',
  },
  {
    name: 'PairFile',
    ui: () => rootPage(PairFile, 'PairFile', { text: pairFileText }),
    ready: studio.name,
  },
  {
    name: 'Files',
    ui: () => {
      const fake = createFakeFiles();
      return (
        <FilesProvider files={fake} picker={fake}>
          {tabPage(Files, 'Files')}
        </FilesProvider>
      );
    },
    ready: 'IMG_4021.HEIC',
  },
  ...(['first', 'changed'] as const).map(kind => ({
    name: `HostKeyPrompt ${kind}`,
    ui: () =>
      rootPage(HostKeyPrompt, 'HostKeyPrompt', {
        sshId: 'ssh1',
        serverId: sshServers[1].serverId,
        fp: 'SHA256:nT4kP0aZWq8c',
        keyType: 'ED25519',
        kind,
        ...(kind === 'changed' ? { oldFp: 'SHA256:previous-fixture' } : {}),
      }),
    ready:
      kind === 'first'
        ? '第一次连接 gpu-train-02'
        : 'gpu-train-02 的主机指纹变了',
  })),
  {
    name: 'RootConfirm',
    ui: () =>
      rootPage(RootConfirm, 'RootConfirm', {
        sshId: 'ssh1',
        serverId: sshServers[0].serverId,
        url: 'https://hs.example.com/a/xyz',
        user: 'root',
      }),
    ready: '以 root 登录 prod-api-01 需要再确认一次',
  },
  {
    name: 'AppLockGate',
    ui: () => <AppLockGate />,
    ready: 'Remote Screen 已锁定',
    options: () => {
      const platform = new FakePlatform();
      platform.authResult = 'cancelled';
      return { platform, settings: { 'onboarding.done': true } };
    },
  },
];

export type A11yCase = {
  name: string;
  mount: () => Promise<Awaited<ReturnType<typeof mount>>>;
  target?: string;
};
let restoreDimensions: (() => void) | undefined;

export function setupA11y() {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
  resetSshStore();
}

export async function cleanupA11y() {
  await cleanup();
  clients.splice(0).forEach(client => client.clear());
  resetSshStore();
  restoreDimensions?.();
  restoreDimensions = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
}

function setSize(wide = false) {
  restoreDimensions?.();
  restoreDimensions = wide
    ? mockWindowDimensions({ width: 1024, height: 768 })
    : mockPhoneDimensions();
}

export async function mountPage(page: Page) {
  setSize(page.wide);
  const view = await mount(page.ui(), page.options?.());
  await view.findAllByText(page.ready);
  return view;
}

export const SESSION_END_CODES = [
  101, 110, 120, 130, 140, 150, 160, 161, 170, 190, 171, 191, 180, 181, 142,
];

export const endedCases: A11yCase[] = SESSION_END_CODES.map(code => ({
  name: `SessionEnded E${code}`,
  mount: () =>
    mountPage({
      name: `SessionEnded E${code}`,
      ui: () =>
        rootPage(SessionEnded, 'SessionEnded', { hostId: studio.hostId, code }),
      ready: code === 180 ? '这台设备已被作废' : /Studio Mac mini/,
    }),
}));

const panels = [
  { panel: 'toolbar', trigger: null, target: 'layer-toolbar' },
  { panel: 'quality', trigger: 'toolbar-quality', target: 'quality-sheet' },
  { panel: 'displays', trigger: 'toolbar-displays', target: 'display-sheet' },
  {
    panel: 'clipboard',
    trigger: 'toolbar-clipboard',
    target: 'clipboard-sheet',
  },
  { panel: 'more', trigger: 'toolbar-more', target: 'more-sheet' },
  { panel: 'keyboard', trigger: 'toolbar-keyboard', target: 'a11y-keyboard' },
  { panel: 'coach', trigger: 'toolbar-more', target: 'gesture-coach' },
];

export const sessionCases = (['Mac', 'Windows'] as const).flatMap(os =>
  panels
    .filter(
      ({ panel }) =>
        os === 'Mac' || ['toolbar', 'quality', 'more'].includes(panel),
    )
    .flatMap(({ panel, trigger, target }) =>
      [false, true].map(wide => ({
        name: `Session ${os} ${panel} ${wide ? 'iPad' : 'phone'}`,
        panel,
        target,
        mount: async () => {
          setSize(wide);
          const host = os === 'Mac' ? studio : office;
          const session = {
            ...liveSession,
            id: os === 'Mac' ? liveSession.id : 'office-a11y',
            hostId: host.hostId,
            hostName: host.name,
          };
          const view = await mount(
            rootPage(Session, 'Session', {
              hostId: host.hostId,
              sessionId: session.id,
            }),
            {
              core: createFakeCore({ sessions: [session] }),
            },
          );
          await view.findByTestId('toolbar-disconnect');
          await act(() =>
            view.core.emit({
              type: 'session.displays',
              sessionId: session.id,
              displays: host.displays,
              currentId: 1,
            }),
          );
          if (panel === 'clipboard') {
            await act(() => {
              view.core.emit({
                type: 'session.clipboard',
                sessionId: session.id,
                direction: 'toHost',
                text: 'pwd',
                at: FIXTURE_NOW - 2_000_000,
              });
              view.core.emit({
                type: 'session.clipboard',
                sessionId: session.id,
                direction: 'fromHost',
                text: 'hostname',
                at: FIXTURE_NOW - 1_000_000,
              });
            });
          }
          if (trigger !== null) {
            if (panel === 'clipboard' && !wide) {
              await fireEvent.press(view.getByTestId('toolbar-more'));
              await view.findByTestId('more-sheet');
              await fireEvent.press(
                view.getByRole('button', { name: '剪贴板' }),
              );
            } else {
              await fireEvent.press(view.getByTestId(trigger));
            }
            if (panel === 'coach') {
              await view.findByTestId('more-sheet');
              await fireEvent.press(
                view.getByRole('button', { name: '手势说明' }),
              );
            }
          }
          await view.findByTestId(target);
          if (panel === 'clipboard') {
            await view.findByTestId('clipboard-recent');
          }
          return view;
        },
      })),
    ),
);

export const screenCases: A11yCase[] = [
  ...pages.map(page => ({ name: page.name, mount: () => mountPage(page) })),
  ...sessionCases,
  ...endedCases,
];

// Same complete SessionCtx as the six overlay unit tests; no session state-machine shortcuts.
function sessionContext(): SessionCtx {
  return {
    hostId: studio.hostId,
    hostName: studio.name,
    os: 'macos',
    host: studio,
    myDeviceId: 'phone',
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: {
      ...liveSession,
      phase: 'connecting',
      role: 'controller',
      controller: null,
      viewers: [],
      inputPausedLocal: false,
      stats: null,
      displays: [],
      dial: [],
      approvalTimeoutS: 30,
      derpRegion: null,
      reconnectReason: null,
      idleWarning: null,
      controlRequest: null,
      lastError: null,
      clipboard: [],
    },
    view: {
      overlay: 'connecting',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: false,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: false,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '',
    },
    isPad: false,
    reduceMotion: true,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: FIXTURE_NOW / 1000,
    phaseAtMs: {},
    firstFrameAtMs: null,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
  };
}

export const overlayCases: Page[] = [
  {
    name: 'ConnectingOverlay',
    ui: () => <ConnectingOverlay ctx={sessionContext()} />,
    ready: '正在连接 Studio Mac mini',
  },
  {
    name: 'ApprovalOverlay',
    ui: () => {
      const ctx = sessionContext();
      return (
        <ApprovalOverlay
          ctx={{
            ...ctx,
            rec: { ...ctx.rec!, phase: 'pendingApproval' },
            view: { ...ctx.view, overlay: 'approval' },
          }}
        />
      );
    },
    ready: '等待 Mac 上允许',
  },
  {
    name: 'ReconnectOverlay',
    ui: () => {
      const ctx = sessionContext();
      return (
        <ReconnectOverlay
          ctx={{
            ...ctx,
            rec: {
              ...ctx.rec!,
              phase: 'reconnecting',
              reconnectReason: 'pathLost',
            },
            view: { ...ctx.view, overlay: 'reconnecting', inputBlocked: true },
            phaseAtMs: { reconnecting: FIXTURE_NOW / 1000 },
          }}
        />
      );
    },
    ready: '正在重新连接',
  },
  {
    name: 'IdleOverlay',
    ui: () => {
      const ctx = sessionContext();
      return (
        <IdleOverlay
          ctx={{
            ...ctx,
            rec: {
              ...ctx.rec!,
              phase: 'streaming',
              idleWarning: {
                secondsLeft: 60,
                lockOnEnd: true,
                atUs: FIXTURE_NOW,
              },
            },
            view: {
              ...ctx.view,
              overlay: 'none',
              idle: true,
              inputBlocked: true,
            },
          }}
        />
      );
    },
    ready: '长时间没有操作了',
  },
  {
    name: 'SharedOverlay viewer',
    ui: () => {
      const ctx = sessionContext();
      return (
        <SharedOverlay
          ctx={{
            ...ctx,
            rec: {
              ...ctx.rec!,
              phase: 'streaming',
              role: 'viewer',
              controller: {
                deviceId: 'pad',
                deviceName: '小明的 iPad',
                user: SESSION_USER,
                path: 'tailnetDirect',
                since: FIXTURE_NOW,
              },
              viewers: [
                {
                  deviceId: 'phone',
                  deviceName: '小明的 iPhone',
                  user: SESSION_USER,
                  path: 'tailnetDirect',
                  since: FIXTURE_NOW,
                },
              ],
            },
            view: { ...ctx.view, overlay: 'none', role: 'viewer' },
          }}
        />
      );
    },
    ready: '接管控制',
  },
  {
    name: 'SharedOverlay watchers',
    ui: () => {
      const ctx = sessionContext();
      return (
        <>
          <SessionToolbar ctx={ctx} />
          <SharedOverlay
            ctx={{
              ...ctx,
              rec: {
                ...ctx.rec!,
                phase: 'streaming',
                viewers: [
                  {
                    deviceId: 'pad',
                    deviceName: '小明的 iPad',
                    user: SESSION_USER,
                    path: 'tailnetDirect',
                    since: FIXTURE_NOW,
                  },
                ],
              },
              view: { ...ctx.view, overlay: 'none' },
            }}
          />
        </>
      );
    },
    ready: '小明的 iPad 在观看',
  },
  {
    name: 'LocalUseBar',
    ui: () => {
      const ctx = sessionContext();
      return (
        <>
          <SessionToolbar ctx={ctx} />
          <LocalUseBar
            ctx={{
              ...ctx,
              view: {
                ...ctx.view,
                overlay: 'none',
                localUse: true,
                inputBlocked: true,
              },
            }}
          />
        </>
      );
    },
    ready: '对方停手 1 秒后自动交还',
  },
];
