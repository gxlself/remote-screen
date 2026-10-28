import { useEffect, useId, useRef, useState, type JSX } from 'react';

import type { CoreEventOf, GoodbyeReason } from '@app/native/rscore/types';
import { Banner, Button } from '@host-ui/components';

import { useCore } from '../../core/CoreContext';
import { useCoreEvents, useSessionEvents } from '../hooks';

import styles from './session.module.css';
import { text } from './text';

/** At most this many「连接已断开」banners stay on screen; older ones drop off. */
export const MAX_ENDED = 3;

type SessionError = CoreEventOf<'session.error'>;
type ControlRequest = CoreEventOf<'session.controlRequest'> & { key: string; deadline: number };
type EndedNotice = { id: number; hostId: string; reason: string | null };

/** Named reasons get their own text; anything else (`background` never reaches here) shows the message. */
export function endedReason(error: SessionError | undefined): string | null {
  if (error === undefined) {
    return null;
  }
  const reasons: Partial<Record<GoodbyeReason, string>> = text.reasons;
  const named = error.reason === undefined ? undefined : reasons[error.reason];
  return named ?? error.message ?? null;
}

export type SessionNoticesProps = {
  /** hostId → name, to say which computer a session belonged to. */
  hostNames: ReadonlyMap<string, string>;
};

/**
 * Sessions live in native windows; the main window says when one ended unexpectedly and asks
 * about control requests from other devices.
 */
export function SessionNotices({ hostNames }: SessionNoticesProps): JSX.Element | null {
  const core = useCore();
  const titleId = useId();
  const errors = useRef(new Map<string, SessionError>());
  const nextId = useRef(0);
  const [ended, setEnded] = useState<EndedNotice[]>([]);
  const [requests, setRequests] = useState<ControlRequest[]>([]);
  const [now, setNow] = useState(() => Date.now());

  useCoreEvents((event) => {
    if (event.type === 'session.error') {
      errors.current.set(event.sessionId, event);
    } else if (event.type === 'session.controlRequest') {
      const at = Date.now();
      const key = `${event.sessionId}/${event.requester.deviceId}`;
      setNow(at);
      setRequests((current) => [
        ...current.filter((r) => r.key !== key),
        { ...event, key, deadline: at + event.timeoutS * 1000 },
      ]);
    }
  });

  useSessionEvents((event) => {
    if (event.type !== 'ended') {
      return;
    }
    const error = errors.current.get(event.sessionId);
    errors.current.delete(event.sessionId);
    if (event.byUser) {
      return;
    }
    nextId.current += 1;
    const notice: EndedNotice = {
      id: nextId.current,
      hostId: event.hostId,
      reason: endedReason(error),
    };
    setEnded((current) => [notice, ...current].slice(0, MAX_ENDED));
  });

  const asking = requests.length > 0;
  useEffect(() => {
    if (!asking) {
      return;
    }
    const timer = setInterval(() => {
      const at = Date.now();
      setNow(at);
      // Unanswered requests just close; the Host times them out on its side.
      setRequests((current) => current.filter((r) => r.deadline > at));
    }, 1000);
    return () => clearInterval(timer);
  }, [asking]);

  const respond = (request: ControlRequest, allow: boolean) => {
    setRequests((current) => current.filter((r) => r.key !== request.key));
    try {
      core.sessions.respondControl(request.sessionId, request.requester.deviceId, allow);
    } catch {
      // A malformed session id: the Host times the request out.
    }
  };

  const request = requests[0];
  if (ended.length === 0 && request === undefined) {
    return null;
  }
  return (
    <>
      {ended.length > 0 ? (
        <div className={styles.notices}>
          {ended.map((notice) => (
            <div className={styles.notice} key={notice.id}>
              <div className={styles.banner}>
                <Banner
                  role="alert"
                  title={text.ended(
                    hostNames.get(notice.hostId) ?? text.unknownHost,
                    notice.reason,
                  )}
                  tone="bad"
                />
              </div>
              <Button
                onClick={() => setEnded((current) => current.filter((n) => n.id !== notice.id))}
                size="sm"
                variant="ghost"
              >
                {text.close}
              </Button>
            </div>
          ))}
        </div>
      ) : null}
      {request === undefined ? null : (
        <div className={styles.scrim}>
          <div
            aria-labelledby={titleId}
            aria-modal="true"
            className={styles.dialog}
            role="alertdialog"
          >
            <p className={styles.message} id={titleId}>
              {text.controlRequest(
                request.requester.deviceName,
                request.requester.user,
                Math.max(0, Math.ceil((request.deadline - now) / 1000)),
              )}
            </p>
            <div className={styles.actions}>
              <Button autoFocus onClick={() => respond(request, false)} variant="secondary">
                {text.deny}
              </Button>
              <Button onClick={() => respond(request, true)}>{text.allow}</Button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
