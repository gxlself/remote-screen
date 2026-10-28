import { useState, type JSX } from 'react';

import type { PageProps } from '../../app/router';
import { zh } from '../../i18n';
import { useIpc, useIpcQuery } from '../../ipc';
import type { SessionSummary } from '../../ipc';
import { formatElapsed, useNow } from '../../lib';

import styles from './capsule.module.css';

type RecordingSession = SessionSummary & { recording?: boolean };

function mainCopy(controller: SessionSummary | undefined, viewers: SessionSummary[]): string {
  if (controller !== undefined) {
    return zh.capsule.controlling(controller.device_name);
  }

  const viewer = viewers[0];
  if (viewers.length === 1 && viewer !== undefined) {
    return zh.capsule.watchingOne(viewer.device_name);
  }
  return zh.capsule.watchingMany(viewers.length);
}

function viewerCopy(viewers: SessionSummary[]): string {
  const viewer = viewers[0];
  return viewers.length === 1 && viewer !== undefined
    ? zh.capsule.viewerOne(viewer.device_name)
    : zh.capsule.viewerMany(viewers.length);
}

function recordingCopy(recorders: RecordingSession[]): string | number {
  const recorder = recorders[0];
  return recorders.length === 1 && recorder !== undefined ? recorder.device_name : recorders.length;
}

function EyeIcon(): JSX.Element {
  return (
    <svg aria-hidden="true" className={styles.eye} viewBox="0 0 24 24" fill="none">
      <path
        d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7-10-7-10-7z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="2" />
    </svg>
  );
}

export default function CapsulePage(_props: PageProps): JSX.Element {
  const ipc = useIpc();
  const { data } = useIpcQuery('sessions.list', { refetchOn: ['event.sessionsChanged'] });
  const now = useNow();
  const [disconnecting, setDisconnecting] = useState(false);

  const sessions = (data?.sessions ?? []) as RecordingSession[];
  if (sessions.length === 0) {
    return <main data-page="capsule" />;
  }

  const controller = sessions.find((session) => session.role === 'controller');
  const viewers = sessions.filter((session) => session.role === 'viewer');
  const recorders = sessions.filter((session) => session.recording === true);
  const startedAtUs =
    controller?.connected_at_us ?? Math.min(...sessions.map((session) => session.connected_at_us));
  const elapsed = formatElapsed(now - startedAtUs / 1000);
  const hasViewerChip = controller !== undefined && viewers.length > 0;

  const handleDisconnect = async (): Promise<void> => {
    if (disconnecting) {
      return;
    }

    setDisconnecting(true);
    const sessionIds = sessions.map((session) => session.session_id);
    try {
      for (const sessionId of sessionIds) {
        try {
          await ipc.call('sessions.disconnect', { session_id: sessionId });
        } catch {
          console.warn('sessions.disconnect failed');
        }
      }
    } finally {
      setDisconnecting(false);
    }
  };

  return (
    <main data-page="capsule" className={styles.page}>
      <div className={styles.pill} role="status" data-tauri-drag-region>
        <span aria-hidden="true" className={styles.dot} />
        <span className={styles.message}>{mainCopy(controller, viewers)}</span>
        {recorders.length > 0 ? (
          <span
            className={styles.recording}
            role="status"
            aria-label={zh.capsule.recordingA11y(recordingCopy(recorders))}
          >
            <span className={styles.recDot} aria-hidden="true" />
            {zh.capsule.recording}
          </span>
        ) : null}
        <span className={styles.elapsed}>{elapsed}</span>
        {hasViewerChip ? (
          <span className={styles.viewers}>
            <EyeIcon />
            {viewerCopy(viewers)}
          </span>
        ) : null}
        <button
          type="button"
          className={styles.disconnect}
          disabled={disconnecting}
          onClick={() => void handleDisconnect()}
        >
          {sessions.length === 1 ? zh.common.disconnect : zh.common.disconnectAll}
        </button>
      </div>
    </main>
  );
}
