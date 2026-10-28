import { Button, DeviceGlyph, Tag } from '../../components';
import { zh } from '../../i18n';
import { formatElapsed } from '../../lib';

import styles from './Menu.module.css';
import type { SessionSummary } from '../../ipc';

interface SessionCardBase {
  session: SessionSummary;
  now: number;
  disabled: boolean;
}

interface ControllerCardProps extends SessionCardBase {
  onDisconnect: (sessionId: string) => void;
  onViewOnly: (sessionId: string) => void;
}

interface ViewerCardProps extends SessionCardBase {
  onDisconnect: (sessionId: string) => void;
}

function connectedFor(session: SessionSummary, now: number): string {
  const connectedAtMs = session.connected_at_us / 1000;
  return zh.menu.connectedFor(formatElapsed(now - connectedAtMs));
}

export function ControllerCard({
  session,
  now,
  disabled,
  onDisconnect,
  onViewOnly,
}: ControllerCardProps) {
  return (
    <article className={styles.controllerCard} data-session-id={session.session_id}>
      <div className={styles.sessionHead}>
        <DeviceGlyph platform="unknown" size={30} />
        <div className={styles.sessionIdentity}>
          <div className={styles.sessionName}>{session.device_name}</div>
          <div className={styles.sessionElapsed}>{connectedFor(session, now)}</div>
        </div>
        <Tag tone="controlling">{zh.menu.controlling}</Tag>
      </div>
      <div className={styles.sessionActions}>
        <Button
          disabled={disabled}
          onClick={() => onViewOnly(session.session_id)}
          variant="secondary"
        >
          {zh.menu.switchToViewOnly}
        </Button>
        <Button
          disabled={disabled}
          onClick={() => onDisconnect(session.session_id)}
          variant="danger"
        >
          {zh.menu.disconnect}
        </Button>
      </div>
    </article>
  );
}

export function ViewerCard({ session, now, disabled, onDisconnect }: ViewerCardProps) {
  return (
    <article className={styles.viewerCard} data-session-id={session.session_id}>
      <div className={styles.viewerBody}>
        <div className={styles.sessionIdentity}>
          <div className={styles.viewerNameRow}>
            <span className={styles.sessionName}>{session.device_name}</span>
            <Tag tone="neutral">{zh.menu.watching}</Tag>
          </div>
          <div className={styles.sessionElapsed}>{connectedFor(session, now)}</div>
        </div>
        <Button
          disabled={disabled}
          onClick={() => onDisconnect(session.session_id)}
          size="sm"
          variant="danger"
        >
          {zh.menu.disconnect}
        </Button>
      </div>
    </article>
  );
}
