import type React from 'react';
import type { SshId } from '../../native/rscore/types';
import { TerminalPane } from '../../native/terminal/TerminalPane';

export type TerminalHandle = {
  sendSequence(s: string): void;
  copySelection(): void;
  paste(): void;
};

export type TerminalSlotProps = {
  sshId: SshId | null;
  fontSize: number;
  theme: 'night';
  accessibilityLabel: string;
  active: boolean;
  accessory?: React.ReactNode;
  onCopied?: () => void;
  ref?: React.Ref<TerminalHandle>;
};

export function TerminalSlot(p: TerminalSlotProps): React.JSX.Element {
  return <TerminalPane {...p} />;
}
