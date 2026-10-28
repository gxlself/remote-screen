import type { ComponentType } from 'react';
import PingHarness from './PingHarness';
import NetInfoHarness from './NetInfoHarness';
import LocalAuthHarness from './LocalAuthHarness';
import SecureKeyHarness from './SecureKeyHarness';
import SessionViewHarness from './SessionViewHarness';
import KeyboardHarness from './KeyboardHarness';
import SystemHarness from './SystemHarness';
import LifecycleHarness from './LifecycleHarness';
import FilesHarness from './FilesHarness';
import SelfTestHarness from './SelfTestHarness';
import TerminalHarness from './TerminalHarness';
import AudioHarness from './AudioHarness';

export type HarnessProps = {
  arg?: string;
  log: (tag: string, payload: Record<string, unknown>) => void;
};

export const harnesses: Record<string, ComponentType<HarnessProps>> = {
  ping: PingHarness,
  netinfo: NetInfoHarness,
  localauth: LocalAuthHarness,
  securekey: SecureKeyHarness,
  session: SessionViewHarness,
  keyboard: KeyboardHarness,
  system: SystemHarness,
  scanner: require('./ScannerHarness').default,
  lifecycle: LifecycleHarness,
  audio: AudioHarness,
  files: FilesHarness,
  selftest: SelfTestHarness,
  terminal: TerminalHarness,
  ipad: require('./IPadHarness').default,
};
