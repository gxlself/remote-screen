import { useCallback, useEffect, useRef, useState } from 'react';

import type { PageProps } from '../../app/router';
import { Banner, Button, KeyValueList, WindowFrame } from '../../components';
import { zh } from '../../i18n';
import { useIpc, useIpcEvent } from '../../ipc';
import { PAIR_STATUS, type PairingResultEvent } from '../../ipc/events';
import type { PairingStartResult } from '../../ipc/types';
import {
  formatCode,
  formatCountdown,
  formatFp,
  hostOfAddr,
  parsePairUrl,
  useRemaining,
  type PairUrl,
} from '../../lib';
import { usePlatform } from '../../platform';

import { QrCode } from './QrCode';
import styles from './pair.module.css';

interface CurrentPairing {
  result: PairingStartResult;
  parsed: PairUrl | null;
  deadlineMs: number;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function spokenCode(code: string): string {
  return code.split('').join(' ');
}

function localDevice(parsed: PairUrl): string {
  const firstAddress = parsed.addrs[0];
  if (firstAddress === undefined) {
    return parsed.name;
  }
  const suffix = parsed.addrs.length > 1 ? zh.pair.addressCount(parsed.addrs.length) : '';
  return `${parsed.name} · ${hostOfAddr(firstAddress)}${suffix}`;
}

function ClockIcon() {
  return (
    <svg aria-hidden="true" height="12" viewBox="0 0 12 12" width="12">
      <circle cx="6" cy="6" fill="none" r="4.75" stroke="currentColor" strokeWidth="1.2" />
      <path
        d="M6 3.6V6l1.65 1"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.2"
      />
    </svg>
  );
}

function InfoIcon() {
  return (
    <svg aria-hidden="true" height="14" viewBox="0 0 14 14" width="14">
      <circle cx="7" cy="7" fill="none" r="5.7" stroke="currentColor" strokeWidth="1.2" />
      <path d="M7 6.2v3.2" stroke="currentColor" strokeLinecap="round" strokeWidth="1.2" />
      <circle cx="7" cy="4.2" fill="currentColor" r="0.75" />
    </svg>
  );
}

export default function PairPage(_props: PageProps) {
  const ipc = useIpc();
  const platform = usePlatform();
  const [pairing, setPairing] = useState<CurrentPairing | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [pairResult, setPairResult] = useState<PairingResultEvent | null>(null);
  const [busy, setBusy] = useState(false);

  const currentRef = useRef<CurrentPairing | null>(null);
  const busyRef = useRef(false);
  const mountedRef = useRef(false);
  const startedRef = useRef(false);

  const cancelCode = useCallback(
    async (code: string): Promise<void> => {
      try {
        await ipc.call('pairing.cancel', { code });
      } catch {
        return;
      }
    },
    [ipc],
  );

  const startPairing = useCallback(
    async (cancelFirst: boolean): Promise<void> => {
      if (busyRef.current) {
        return;
      }

      busyRef.current = true;
      setBusy(true);
      setPairResult(null);
      setStartError(null);

      const previous = currentRef.current;
      if (cancelFirst && previous !== null) {
        currentRef.current = null;
        setPairing(null);
        await cancelCode(previous.result.code);
      }

      try {
        const result = await ipc.call('pairing.start');
        if (!mountedRef.current) {
          void ipc.call('pairing.cancel', { code: result.code }).catch(() => undefined);
          return;
        }

        const next: CurrentPairing = {
          result,
          parsed: parsePairUrl(result.qr_url),
          deadlineMs: result.expires_at / 1000,
        };
        currentRef.current = next;
        setPairing(next);
      } catch (error) {
        if (mountedRef.current) {
          setStartError(messageOf(error));
        }
      } finally {
        busyRef.current = false;
        if (mountedRef.current) {
          setBusy(false);
        }
      }
    },
    [cancelCode, ipc],
  );

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (startedRef.current) {
      return;
    }
    startedRef.current = true;
    void startPairing(false);
  }, [startPairing]);

  useEffect(() => {
    const cancelCurrent = (): void => {
      const current = currentRef.current;
      if (current === null) {
        return;
      }
      currentRef.current = null;
      void ipc.call('pairing.cancel', { code: current.result.code }).catch(() => undefined);
    };

    window.addEventListener('pagehide', cancelCurrent);
    return () => {
      window.removeEventListener('pagehide', cancelCurrent);
      cancelCurrent();
    };
  }, [ipc]);

  useIpcEvent('event.pairingResult', (event: PairingResultEvent) => {
    const current = currentRef.current;
    if (current === null) {
      return;
    }

    if (current.parsed !== null && event.host_id !== current.parsed.hostId) {
      return;
    }

    setPairResult(event);
  });

  const remaining = useRemaining(pairing === null ? null : pairing.deadlineMs);
  const pairStatus = pairResult?.status;
  const expiredByTime = pairStatus !== PAIR_STATUS.OK && remaining !== null && remaining <= 0;
  const expired = pairStatus === PAIR_STATUS.EXPIRED || expiredByTime;
  const locked = pairStatus === PAIR_STATUS.LOCKED;
  const unavailable = expired || locked;
  const parsed = pairing?.parsed ?? null;
  const countdownText = remaining === null ? null : formatCountdown(remaining);
  const overlayText = locked
    ? zh.pair.expiry.invalid
    : expired
      ? zh.pair.expiry.expiredOverlay
      : null;

  const complete = async (): Promise<void> => {
    if (busyRef.current) {
      return;
    }
    busyRef.current = true;
    setBusy(true);
    const current = currentRef.current;
    if (current !== null) {
      currentRef.current = null;
      await cancelCode(current.result.code);
    }
    try {
      await platform.hideWindow('pair');
    } finally {
      busyRef.current = false;
      if (mountedRef.current) {
        setBusy(false);
      }
    }
  };

  const startFresh = (): void => {
    void startPairing(true);
  };

  let statusBanner = null;
  switch (pairStatus) {
    case PAIR_STATUS.WAITING_LOCAL:
      statusBanner = <Banner tone="info" title={zh.pair.status.waitingLocal} />;
      break;
    case PAIR_STATUS.OK:
      statusBanner = (
        <Banner tone="good" title={zh.pair.status.ok}>
          {zh.pair.status.okHint}
        </Banner>
      );
      break;
    case PAIR_STATUS.BAD_CODE:
      statusBanner = (
        <Banner tone="bad" title={zh.pair.status.badCode(pairResult?.attempts_left ?? 0)} />
      );
      break;
    case PAIR_STATUS.LOCKED:
      statusBanner = <Banner tone="bad" title={zh.pair.status.locked} />;
      break;
    case PAIR_STATUS.DENIED:
      statusBanner = <Banner tone="info" title={zh.pair.status.denied} />;
      break;
    case PAIR_STATUS.DISABLED_ON_PATH:
      statusBanner = (
        <Banner tone="bad" title={zh.pair.status.disabledOnPath}>
          <button
            className={styles.linkButton}
            onClick={() => {
              void platform.showWindow('settings', 'settings/network');
            }}
            type="button"
          >
            {zh.pair.status.networkAccess}
          </button>
        </Banner>
      );
      break;
    default:
      statusBanner = null;
  }

  return (
    <WindowFrame
      footer={
        <div className={styles.footerActions}>
          {unavailable ? null : (
            <Button disabled={busy} onClick={startFresh} variant="secondary">
              {zh.pair.newCode}
            </Button>
          )}
          <Button
            disabled={busy}
            onClick={unavailable ? startFresh : () => void complete()}
            variant="primary"
          >
            {unavailable ? zh.pair.newCode : zh.pair.done}
          </Button>
        </div>
      }
      title={zh.pair.title}
      titleId="pair-title"
    >
      <div className={styles.layout} data-page="pair">
        <div className={styles.sidebar}>
          <div className={styles.qrFrame}>
            {pairing === null ? (
              <div className={styles.qrPlaceholder}>
                {busy ? zh.pair.loading : startError === null ? zh.pair.loading : zh.pair.noCode}
              </div>
            ) : (
              <div
                className={[styles.qrCode, unavailable ? styles.qrInactive : undefined]
                  .filter(Boolean)
                  .join(' ')}
              >
                <QrCode size={220} value={pairing.result.qr_url} />
              </div>
            )}
            {overlayText !== null ? (
              <div className={styles.qrOverlay}>
                <span className={styles.qrOverlayLabel}>{overlayText}</span>
              </div>
            ) : null}
          </div>

          <div className={styles.expiryRow}>
            <ClockIcon />
            {expired ? (
              <span>{zh.pair.expiry.expired}</span>
            ) : countdownText !== null ? (
              <>
                <span aria-hidden="true" className={styles.expiryVisual}>
                  <span className={styles.expiryTime}>{countdownText}</span>
                  <span>{zh.pair.expiry.suffix}</span>
                </span>
                <span className={styles.srOnly}>{`${countdownText}${zh.pair.expiry.suffix}`}</span>
              </>
            ) : (
              <span>{startError === null ? zh.pair.loading : zh.pair.noCode}</span>
            )}
          </div>
        </div>

        <div className={styles.main}>
          <div className={styles.status} role="status">
            {statusBanner}
          </div>

          {startError !== null ? (
            <Banner role="alert" tone="bad">
              <div className={styles.errorRow}>
                <span className={styles.errorText}>{zh.pair.startFailed(startError)}</span>
                <Button
                  disabled={busy}
                  onClick={() => {
                    void startPairing(false);
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {zh.common.retry}
                </Button>
              </div>
            </Banner>
          ) : null}

          <ol className={styles.steps}>
            <li className={styles.step}>
              <span className={styles.stepNumber}>1</span>
              <span className={styles.stepText}>{zh.pair.instructions.open}</span>
            </li>
            <li className={styles.step}>
              <span className={styles.stepNumber}>2</span>
              <span className={styles.stepText}>
                {zh.pair.instructions.addPrefix}
                <strong>+</strong>
                {zh.pair.instructions.addSuffix}
              </span>
            </li>
            <li className={styles.step}>
              <span className={styles.stepNumber}>3</span>
              <span className={styles.stepText}>{zh.pair.instructions.scan}</span>
            </li>
          </ol>

          {pairing !== null ? (
            <>
              <div className={styles.codeBlock}>
                <div className={styles.codeLabel}>{zh.pair.codeLabel}</div>
                <div aria-label={spokenCode(pairing.result.code)} className={styles.code}>
                  {formatCode(pairing.result.code)}
                </div>
              </div>

              {parsed !== null ? (
                <div className={styles.details}>
                  <KeyValueList
                    items={[
                      { key: zh.pair.local, value: localDevice(parsed), mono: true },
                      { key: zh.pair.fingerprint, value: formatFp(parsed.fp), mono: true },
                    ]}
                    keyWidth={76}
                  />
                </div>
              ) : null}
            </>
          ) : null}

          <div className={styles.info}>
            <span className={styles.infoIcon}>
              <InfoIcon />
            </span>
            <span>{zh.pair.info}</span>
          </div>
        </div>
      </div>
    </WindowFrame>
  );
}
