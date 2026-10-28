import { common } from './zh/common';
import { onboarding } from './zh/onboarding';
import { onboardDiscover } from './zh/onboardDiscover';
import { directory } from './zh/directory';
import { home } from './zh/home';
import { deviceSheet } from './zh/deviceSheet';
import { deviceRoutes } from './zh/deviceRoutes';
import { pairing } from './zh/pairing';
import { pairAdd } from './zh/pairAdd';
import { pairWaiting } from './zh/pairWaiting';
import { sessionHud } from './zh/sessionHud';
import { session } from './zh/session';
import { sessionToolbar } from './zh/sessionToolbar';
import { sessionConnect } from './zh/sessionConnect';
import { qualityDisplay } from './zh/qualityDisplay';
import { clipboardMore } from './zh/clipboardMore';
import { coach } from './zh/coach';
import { localIdleReconnect } from './zh/localIdleReconnect';
import { sharedRelayTcp } from './zh/sharedRelayTcp';
import { sessionEnded } from './zh/sessionEnded';
import { errBoards1 } from './zh/errBoards1';
import { errBoards2 } from './zh/errBoards2';
import { errBoards3 } from './zh/errBoards3';
import { sessions } from './zh/sessions';
import { addServer } from './zh/addServer';
import { terminal } from './zh/terminal';
import { sshPrompt } from './zh/sshPrompt';
import { snippets } from './zh/snippets';
import { sshKeys } from './zh/sshKeys';
import { settings } from './zh/settings';
import { appLock } from './zh/appLock';
import { networks } from './zh/networks';
import { tailnet } from './zh/tailnet';
import { addByAddress } from './zh/addByAddress';
import { diagnostics } from './zh/diagnostics';
import { ipadHome } from './zh/ipadHome';
import { ipadSession } from './zh/ipadSession';
import { fileTransfers } from './zh/fileTransfers';
import { filesBrowse } from './zh/filesBrowse';
import { files } from './zh/files';
import { vdisplay } from './zh/vdisplay';
import { pip } from './zh/pip';
import { recording } from './zh/recording';
import { recordings } from './zh/recordings';
import { adapters } from './zh/adapters';
import { native } from './zh/native';

export const zh = {
  appName: 'Remote Screen',
  dev: {
    subtitle: 'RN 0.88 · Fabric + UniFFI TurboModule + C ABI',
    selfTest: 'Rust 自检',
    waiting: '等待事件…',
    runSelfTest: '运行自检（5 秒事件）',
    nativeVideo: '原生视频',
    hostIp: 'Host IP',
    hostPort: '端口',
    hostIpAccessibility: 'Host IP 地址',
    hostPortAccessibility: 'Host 端口',
    connect: '连接',
    disconnect: '断开',
    idle: '尚未连接，请连接本地开发 Host。',
    decodeHint:
      'decodeP50Ms = -1 表示尚无解码样本；预热 2 秒。Fake Host 仅能验证收帧，其数据不能解码。',
  },
  common,
  onboarding,
  onboardDiscover,
  directory,
  home,
  deviceSheet,
  deviceRoutes,
  pairing,
  pairAdd,
  pairWaiting,
  sessionHud,
  session,
  sessionToolbar,
  sessionConnect,
  qualityDisplay,
  clipboardMore,
  coach,
  localIdleReconnect,
  sharedRelayTcp,
  sessionEnded,
  errBoards1,
  errBoards2,
  errBoards3,
  sessions,
  addServer,
  terminal,
  sshPrompt,
  sshKeys,
  settings,
  appLock,
  networks,
  tailnet,
  addByAddress,
  diagnostics,
  ipadHome,
  ipadSession,
  snippets,
  fileTransfers,
  filesBrowse,
  files,
  vdisplay,
  pip,
  recording,
  recordings,
  adapters,
  native,
} as const;
