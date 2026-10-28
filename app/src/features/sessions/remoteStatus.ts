import { sessions } from '../../i18n/zh/sessions';
import type {
  AdapterPhase,
  Role,
  SessionPhase,
} from '../../native/rscore/types';

export type RemoteStatus = {
  label: string;
  /** Not streaming yet (or again): amber instead of green. */
  relay: boolean;
};

/** What a live Host session is doing, as the session tab and Home's bar say it. */
export function remoteStatus(
  phase: SessionPhase,
  role: Role | null,
): RemoteStatus {
  switch (phase) {
    case 'connecting':
    case 'authenticating':
    case 'pendingApproval':
      return { label: sessions.remote.status.connecting, relay: true };
    case 'reconnecting':
      return { label: sessions.remote.status.reconnecting, relay: true };
    case 'streaming':
    default:
      return {
        label:
          role === 'viewer'
            ? sessions.remote.status.viewing
            : sessions.remote.status.controlling,
        relay: false,
      };
  }
}

/**
 * A folded-away Screen Sharing / Remote Desktop session (V33). One that
 * dropped while away stays listed until the user returns or disconnects it.
 */
export function adapterStatus(phase: AdapterPhase): RemoteStatus {
  switch (phase) {
    case 'streaming':
      return { label: sessions.remote.status.controlling, relay: false };
    case 'ended':
      return { label: sessions.remote.status.disconnected, relay: true };
    default:
      return { label: sessions.remote.status.connecting, relay: true };
  }
}
