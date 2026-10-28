import type React from 'react';
import type { IconName } from '../../components/icons/Icon';
import type { ToastAction } from '../../components/Toast';
import type { HudModel } from '../../features/session/hud';
import type { SessionRecord } from '../../features/session/store';
import type { SessionView } from '../../features/session/view';
import type { SessionShortcutEvent } from '../../native/keyboard/KeyCapture';
import type {
  RemoteSessionHandle,
  SpaceTile,
} from '../../native/session/RemoteSessionView';
import type {
  Computer,
  Quality,
  SessionId,
  SessionPhase,
} from '../../native/rscore/types';

// 固定的插槽契约：后续任务只替换自己的组件，不改会话外壳。
export type SheetKind =
  | 'quality'
  | 'displays'
  | 'clipboard'
  | 'more'
  /** every open session, to switch to another one. */
  | 'switcher';
export type InputMode = 'trackpad' | 'touch';

export type SessionCtx = {
  hostId: string;
  hostName: string;
  os: 'macos' | 'windows';
  host: Computer | undefined;
  myDeviceId: string | null;
  sessionId: SessionId | null;
  videoRef: React.RefObject<RemoteSessionHandle | null>;
  rec: SessionRecord | undefined;
  view: SessionView;
  hud: HudModel;
  isPad: boolean;
  reduceMotion: boolean;
  mode: InputMode;
  setMode(m: InputMode): void;
  quality: Quality;
  setQuality(q: Quality): void;
  startedAtMs: number;
  phaseAtMs: Partial<Record<SessionPhase, number>>;
  firstFrameAtMs: number | null;
  openSheet(k: SheetKind): void;
  closeSheet(): void;
  openCoach(): void;
  closeCoach(): void;
  keyboardOpen: boolean;
  toggleKeyboard(): void;
  toolbarHidden: boolean;
  collapse(): void;
  disconnect(): void;
  cancelConnect(): void;
  openTerminal(): void;
  showToast(
    message: string,
    opts?: {
      tone?: 'neutral' | 'good' | 'bad';
      icon?: IconName;
      /** 可选的动作按钮（重试、撤销），带动作时 Toast 停 6 秒。 */
      action?: ToastAction;
    },
  ): void;
};

export type SlotProps = { ctx: SessionCtx };
export type SheetProps = { ctx: SessionCtx; onClose: () => void };
export type VideoSlotProps = {
  ref?: React.Ref<RemoteSessionHandle>;
  sessionId: SessionId | null;
  mode: InputMode;
  reduceMotion: boolean;
  toolbarCollapsed: boolean;
  keyboardInset: number;
  accessibilityLabel: string;
  onGesture?: (e: { kind: string }) => void;
  onFirstFrame?: () => void;
  /** open computers in swipe order and the switch events. */
  spaces?: readonly SpaceTile[];
  showSnapshot?: boolean;
  onSpaceSwipe?: (e: { phase: string }) => void;
  onSpaceSwitch?: (e: { sessionId: string }) => void;
};
export type KeyboardSlotProps = {
  open: boolean;
  sessionId: SessionId | null;
  os: 'macos' | 'windows';
  leading?: React.ReactNode;
  onClose: () => void;
  /** ⌃⌥⌘ session shortcuts from a hardware keyboard. */
  onShortcut?: (event: SessionShortcutEvent) => void;
};
