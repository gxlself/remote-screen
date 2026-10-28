import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';

import type { PageProps } from '../../app/router';
import {
  AppGlyph,
  Banner,
  Button,
  Checkbox,
  CountdownRing,
  DeviceGlyph,
  KeyValueList,
  RadioGroup,
  Tag,
  type KeyValueItem,
} from '../../components';
import { zh } from '../../i18n';
import { useIpc, useIpcConnected, useIpcEvent, useIpcQuery } from '../../ipc';
import type { ApprovalRequest, IpcError, Permission } from '../../ipc';
import { formatFp, pathLabel, secondsLeft, useRemaining } from '../../lib';
import { usePlatform } from '../../platform';

import styles from './confirm.module.css';

const copy = zh.confirm;

type PermissionChoice = 'control' | 'view_only';

interface QueuedRequest {
  request: ApprovalRequest;
  receivedAt: number;
}

interface RuntimeApprovalFields {
  node_name?: unknown;
  peer?: unknown;
  peer_addr?: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function nonEmpty(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function runtimeFields(request: ApprovalRequest): RuntimeApprovalFields {
  return request as ApprovalRequest & RuntimeApprovalFields;
}

function deviceNodeName(request: ApprovalRequest): string | null {
  const fields = runtimeFields(request);
  const explicit = nonEmpty(fields.node_name);
  if (explicit !== null) {
    return explicit;
  }
  return isRecord(fields.peer) ? nonEmpty(fields.peer.node) : null;
}

function devicePeerAddr(request: ApprovalRequest): string | null {
  const fields = runtimeFields(request);
  const explicit = nonEmpty(fields.peer_addr);
  if (explicit !== null) {
    return explicit;
  }
  if (!isRecord(fields.peer)) {
    return null;
  }
  return nonEmpty(fields.peer.addr) ?? nonEmpty(fields.peer.peer_addr);
}

function requestPath(request: ApprovalRequest): string | null {
  const direct = nonEmpty(request.path);
  if (direct !== null) {
    return direct;
  }
  const peer = runtimeFields(request).peer;
  return isRecord(peer) ? nonEmpty(peer.path) : null;
}

function requestReason(request: ApprovalRequest): string | null {
  const first = request.reasons[0];
  if (first === undefined) {
    return null;
  }
  const normalized = first.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
  switch (normalized) {
    case 'attended':
    case 'attended_mode':
      return 'attended';
    case 'intranet':
    case 'intranet_confirmation':
      return 'intranet';
    case 'localactivity':
    case 'local_activity':
      return 'local_activity';
    case 'nodemismatch':
    case 'node_mismatch':
      return 'node_mismatch';
    case 'devicerequiresconfirmation':
    case 'device_requires_confirmation':
      return 'device_requires_confirmation';
    default:
      return normalized;
  }
}

function isTakeover(request: ApprovalRequest): boolean {
  return request.kind === 'takeover' || String(request.kind) === 'control';
}

function deviceLabel(request: ApprovalRequest): string {
  const name = request.device_name.trim();
  return name.length > 0 ? name : copy.unknownDevice;
}

function responsePermission(choice: PermissionChoice, unattended: boolean): Permission {
  if (choice === 'view_only') {
    return 'view_only';
  }
  return unattended ? 'unattended' : 'control';
}

function accountValue(request: ApprovalRequest): ReactNode {
  if (request.whois_login === null || request.whois_login.length === 0) {
    return copy.emptyValue;
  }
  return (
    <span className={styles.account}>
      <span>{request.whois_login}</span>
      <Tag tone="good">{copy.tailnetVerified}</Tag>
    </span>
  );
}

export function approvalDeviceValue(request: ApprovalRequest): string | null {
  const node = deviceNodeName(request);
  const addr = devicePeerAddr(request);
  if (node !== null && addr !== null) {
    return `${node} · ${addr}`;
  }
  return node ?? addr;
}

function detailItems(request: ApprovalRequest): Array<KeyValueItem> {
  const path = requestPath(request);
  return [
    { key: copy.account, value: accountValue(request) },
    { key: copy.device, value: approvalDeviceValue(request), mono: true },
    { key: copy.network, value: path === null ? null : pathLabel(path) },
    {
      key: copy.fingerprint,
      value: <span className={styles.fingerprint}>{formatFp(request.device_fp)}</span>,
      mono: true,
    },
  ];
}

function connectionDescription(request: ApprovalRequest): {
  description: string;
  bad: boolean;
} {
  switch (requestReason(request)) {
    case 'attended':
      return { description: copy.description.attended, bad: false };
    case 'local_activity':
      return { description: copy.description.localActivity, bad: false };
    case 'intranet':
      return { description: copy.description.intranet, bad: false };
    case 'node_mismatch':
      return { description: copy.description.nodeMismatch, bad: true };
    default:
      return { description: copy.description.fallback, bad: false };
  }
}

function ConfirmationDialog({
  item,
  queueLength,
  onRemove,
  windows,
}: {
  item: QueuedRequest;
  queueLength: number;
  onRemove(requestId: string): void;
  windows: boolean;
}) {
  const request = item.request;
  const requestId = request.request_id;
  const client = useIpc();
  const sessions = useIpcQuery('sessions.list', {
    enabled: isTakeover(request),
    refetchOn: ['event.sessionsChanged'],
  });
  const deadline = item.receivedAt + Math.max(0, request.timeout_s) * 1000;
  const remainingMs = useRemaining(deadline);
  const remaining = remainingMs === null ? 0 : secondsLeft(remainingMs);
  const pairing = request.kind === 'pairing';
  const controller = isTakeover(request)
    ? (sessions.data?.sessions.find((session) => session.role === 'controller')?.device_name ??
      null)
    : null;

  const [choice, setChoice] = useState<PermissionChoice>('control');
  const [unattended, setUnattended] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const denyRef = useRef<HTMLSpanElement | null>(null);
  const pendingRef = useRef(false);
  const mountedRef = useRef(false);

  useLayoutEffect(() => {
    mountedRef.current = true;
    denyRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (remainingMs === null || remainingMs > 0) {
      return;
    }
    onRemove(requestId);
  }, [onRemove, remainingMs, requestId]);

  const respond = useCallback(
    async (allow: boolean) => {
      if (pendingRef.current) {
        return;
      }
      pendingRef.current = true;
      setPending(true);
      setError(null);
      try {
        if (allow && pairing) {
          await client.call('approval.respond', {
            request_id: requestId,
            allow: true,
            permission: responsePermission(choice, unattended),
          });
        } else if (allow) {
          await client.call('approval.respond', {
            request_id: requestId,
            allow: true,
          });
        } else {
          await client.call('approval.respond', {
            request_id: requestId,
            allow: false,
          });
        }
        onRemove(requestId);
      } catch (caught) {
        if (!mountedRef.current) {
          return;
        }
        pendingRef.current = false;
        setPending(false);
        const rpcError = caught as IpcError;
        if (rpcError.code === 'not_found') {
          onRemove(requestId);
          return;
        }
        setError(rpcError.message);
      }
    },
    [choice, client, onRemove, pairing, requestId, unattended],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (event.key !== 'Escape' || event.defaultPrevented || pendingRef.current) {
        return;
      }
      event.preventDefault();
      void respond(false);
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [respond]);

  if (remainingMs !== null && remainingMs <= 0) {
    return null;
  }

  const takeover = isTakeover(request);
  const title = takeover
    ? copy.title.takeover(deviceLabel(request))
    : copy.title.connect(deviceLabel(request));
  const connection = connectionDescription(request);
  const description = takeover
    ? copy.description.takeover(controller)
    : pairing
      ? copy.description.pairing
      : connection.description;
  const descriptionBad = !takeover && !pairing && connection.bad;
  const allowLabel = pairing
    ? choice === 'control'
      ? copy.allowControl
      : copy.allowViewOnly
    : copy.allow;

  return (
    <section
      aria-describedby={`${requestId}-description`}
      aria-labelledby={`${requestId}-title`}
      className={styles.panel}
      role="alertdialog"
    >
      <header className={styles.heading}>
        <span className={styles.glyph}>
          <AppGlyph size={52} />
          <span className={styles.deviceBadge}>
            <DeviceGlyph platform={request.platform} size={26} />
          </span>
        </span>
        <span className={styles.copy}>
          <h1 className={styles.title} id={`${requestId}-title`}>
            {title}
          </h1>
          <p
            className={[styles.description, descriptionBad ? styles.descriptionBad : undefined]
              .filter(Boolean)
              .join(' ')}
            id={`${requestId}-description`}
          >
            {description}
          </p>
        </span>
      </header>

      <div className={styles.details}>
        <KeyValueList items={detailItems(request)} />
      </div>

      {pairing ? (
        <div className={styles.choices}>
          <RadioGroup
            label={copy.permission.label}
            onChange={(next) => {
              setChoice(next);
              if (next === 'view_only') {
                setUnattended(false);
              }
            }}
            options={[
              {
                value: 'control',
                title: copy.permission.control,
                description: copy.permission.controlDescription,
              },
              {
                value: 'view_only',
                title: copy.permission.viewOnly,
                description: copy.permission.viewOnlyDescription,
              },
            ]}
            value={choice}
          />
          <Checkbox
            checked={unattended}
            description={windows ? copy.windows.unattendedHint : undefined}
            disabled={choice === 'view_only' || pending}
            label={copy.permission.unattended}
            onChange={setUnattended}
          />
        </div>
      ) : null}

      {error !== null ? (
        <Banner role="alert" tone="bad">
          {copy.respondFailed(error)}
        </Banner>
      ) : null}

      <footer className={styles.footer}>
        <span className={styles.timer}>
          <span className={styles.countdown}>
            <CountdownRing remaining={remaining} total={request.timeout_s} />
            {copy.countdown(remaining)}
          </span>
          {queueLength > 1 ? (
            <span className={styles.queued}>{copy.queued(queueLength - 1)}</span>
          ) : null}
        </span>
        <span className={styles.actions}>
          <span className={styles.denyFocus} ref={denyRef}>
            <Button disabled={pending} onClick={() => void respond(false)} variant="secondary">
              {zh.common.deny}
            </Button>
          </span>
          <Button disabled={pending} onClick={() => void respond(true)}>
            {allowLabel}
          </Button>
        </span>
      </footer>
      {windows ? <p className={styles.lockScreenNote}>{copy.windows.lockScreenNote}</p> : null}
    </section>
  );
}

export default function ConfirmPage(_props: PageProps) {
  const platform = usePlatform();
  const client = useIpc();
  const connected = useIpcConnected();
  const [queue, setQueue] = useState<Array<QueuedRequest>>([]);
  const requestIdsRef = useRef(new Set<string>());
  const closedRequestIdsRef = useRef(new Set<string>());
  const pendingFetchRef = useRef(0);
  const visibleRef = useRef(false);

  const remove = useCallback((requestId: string) => {
    closedRequestIdsRef.current.add(requestId);
    requestIdsRef.current.delete(requestId);
    setQueue((previous) => previous.filter((item) => item.request.request_id !== requestId));
  }, []);

  const enqueue = useCallback((requests: readonly ApprovalRequest[]) => {
    const receivedAt = Date.now();
    const additions: QueuedRequest[] = [];
    for (const request of requests) {
      if (
        requestIdsRef.current.has(request.request_id) ||
        closedRequestIdsRef.current.has(request.request_id)
      ) {
        continue;
      }
      requestIdsRef.current.add(request.request_id);
      additions.push({ request, receivedAt });
    }
    if (additions.length > 0) {
      setQueue((previous) => [...previous, ...additions]);
    }
  }, []);

  useIpcEvent('event.approvalRequested', (request) => {
    enqueue([request]);
  });

  useIpcEvent('event.approvalClosed', (event) => {
    remove(event.request_id);
  });

  useEffect(() => {
    let active = true;
    let connectionUpdates = 0;
    let lastConnected: boolean | null = null;

    const fetchPending = () => {
      const fetchId = pendingFetchRef.current + 1;
      pendingFetchRef.current = fetchId;
      void client.call('approval.pending').then(
        (result) => {
          if (active && pendingFetchRef.current === fetchId) {
            enqueue(result.requests);
          }
        },
        () => {
          if (active && pendingFetchRef.current === fetchId) {
            console.debug('approval.pending failed');
          }
        },
      );
    };

    const clear = () => {
      pendingFetchRef.current += 1;
      requestIdsRef.current.clear();
      closedRequestIdsRef.current.clear();
      setQueue([]);
    };

    const unsubscribe = client.onConnection((next) => {
      connectionUpdates += 1;
      if (!active) {
        return;
      }
      if (!next) {
        clear();
      } else if (lastConnected !== true) {
        fetchPending();
      }
      lastConnected = next;
    });

    void client.isConnected().then(
      (next) => {
        if (!active || connectionUpdates > 0) {
          return;
        }
        lastConnected = next;
        if (next) {
          fetchPending();
        }
      },
      () => undefined,
    );

    return () => {
      active = false;
      pendingFetchRef.current += 1;
      unsubscribe();
    };
  }, [client, enqueue]);

  const activeQueue = connected ? queue : [];

  useEffect(() => {
    const visible = activeQueue.length > 0;
    if (visible === visibleRef.current) {
      return;
    }
    visibleRef.current = visible;
    if (visible) {
      void platform.showWindow('confirm').catch(() => undefined);
    } else {
      void platform.hideWindow('confirm').catch(() => undefined);
    }
  }, [activeQueue.length, platform]);

  useEffect(() => {
    return () => {
      if (visibleRef.current) {
        visibleRef.current = false;
        void platform.hideWindow('confirm').catch(() => undefined);
      }
    };
  }, [platform]);

  const current = activeQueue[0];
  if (current === undefined) {
    return null;
  }

  return (
    <main className={styles.page} data-page="confirm">
      <ConfirmationDialog
        item={current}
        key={current.request.request_id}
        onRemove={remove}
        queueLength={activeQueue.length}
        windows={platform.os === 'windows'}
      />
    </main>
  );
}
