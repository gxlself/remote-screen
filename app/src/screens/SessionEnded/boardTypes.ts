import type { ComponentType } from 'react';
import type { RootStackParamList } from '../../app/navigation/routes';
import type { Computer } from '../../native/rscore/types';

export type EndAction = {
  label: string;
  onPress: () => void;
  disabled?: boolean;
  loading?: boolean;
  accessibilityHint?: string;
};

export type EndActions = {
  retry(opts?: { viewOnly?: boolean }): void;
  openSession(otherHostId: string): void;
  back(): void;
  openTerminal(serverId: string): void;
  openScreenSharing(): Promise<void>;
  copyText(text: string, toast?: string): void;
  copyCode(): void;
  pair(tab?: 'scan' | 'code'): void;
  openDiagnostics(): void;
  toast(message: string): void;
};

/** null when the directory does not know the computer (V60). */
export type EndOs = 'macos' | 'windows' | null;

export type BoardProps = {
  params: RootStackParamList['SessionEnded'];
  code: number | null;
  host: Computer | undefined;
  hostName: string;
  os: EndOs;
  actions: EndActions;
};

export type BoardComponent = ComponentType<BoardProps>;
