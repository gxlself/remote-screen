import { useEffect, useId, useRef, useState, type JSX } from 'react';

import { toRsCoreError } from '@app/native/rscore/errors';
import { RSP_DEFAULT_PORT } from '@app/native/rscore/RsCoreApi';
import type { PairFileSummary, PairOutcome, PairStep } from '@app/native/rscore/types';
import { Banner, Button, KeyValueList, RadioGroup } from '@host-ui/components';

import { useCore, useDesktop } from '../../core/CoreContext';
import { useCoreEvents } from '../hooks';

import {
  addressTarget,
  codeComplete,
  codeDigits,
  formatTime,
  outcomeTone,
  PAIR_METHODS,
  portValid,
  type PairMethod,
} from './form';
import styles from './pair.module.css';
import { text } from './text';

/** After a successful pairing the page waits this long before going back to the list. */
export const DONE_DELAY_MS = 2000;

function outcomeText(outcome: PairOutcome): string {
  switch (outcome.status) {
    case 'ok':
      return text.outcome.ok(outcome.hostName);
    case 'badCode':
      return text.outcome.badCode(outcome.attemptsLeft);
    case 'locked':
      return text.outcome.locked(outcome.retryAfterS);
    case 'error':
      return text.outcome.error(outcome.message);
    default:
      return text.outcome[outcome.status];
  }
}

export type PairPageProps = {
  onBack: () => void;
};

/** Adds a computer by address or with a pairing file. */
export function PairPage({ onBack }: PairPageProps): JSX.Element {
  const core = useCore();
  const desktop = useDesktop();
  const hintId = useId();
  const [method, setMethod] = useState<PairMethod>('address');
  const [code, setCode] = useState('');
  const [addr, setAddr] = useState('');
  const [port, setPort] = useState('');
  const [file, setFile] = useState<PairFileSummary | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<PairStep | null>(null);
  const [outcome, setOutcome] = useState<PairOutcome | null>(null);
  // Set while a pairing.start* call is outstanding; leaving the page cancels it.
  const pairing = useRef(false);

  useCoreEvents((event) => {
    if (event.type === 'pairing.progress' && pairing.current) {
      setStep(event.step);
    }
  });

  useEffect(
    () => () => {
      if (pairing.current) {
        pairing.current = false;
        core.pairing.cancel();
      }
    },
    [core],
  );

  const succeeded = outcome?.status === 'ok';
  useEffect(() => {
    if (!succeeded) {
      return;
    }
    const timer = setTimeout(onBack, DONE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [succeeded, onBack]);

  const run = async (start: () => Promise<PairOutcome>) => {
    pairing.current = true;
    setBusy(true);
    setStep(null);
    setOutcome(null);
    let result: PairOutcome;
    try {
      result = await start();
    } catch (e) {
      result = { status: 'error', message: toRsCoreError(e).message };
    }
    if (!pairing.current) {
      // The page was left; pairing.cancel() already went out.
      return;
    }
    pairing.current = false;
    setBusy(false);
    setStep(null);
    setOutcome(result);
  };

  const submit = () => {
    switch (method) {
      case 'address':
        void run(() => core.pairing.start(addressTarget(addr, port, code)));
        return;
      case 'file':
        if (file !== null) {
          void run(() => core.pairing.start({ fileId: file.fileId }));
        }
        return;
    }
  };

  const pickFile = async () => {
    setFileError(null);
    try {
      const uri = await desktop.pickPairFile();
      if (uri === null) {
        return;
      }
      setFile(await core.pairing.loadFile({ uri }));
    } catch (e) {
      setFile(null);
      setFileError(text.fileError(toRsCoreError(e).message));
    }
  };

  const cancel = () => {
    if (pairing.current) {
      core.pairing.cancel();
    }
  };

  const back = () => {
    if (pairing.current) {
      pairing.current = false;
      core.pairing.cancel();
    }
    onBack();
  };

  const ready =
    method === 'address'
      ? addr.trim() !== '' && portValid(port) && codeComplete(code)
      : file !== null && !file.expired;
  const locked = busy || succeeded;

  return (
    <section className={styles.page}>
      <header className={styles.header}>
        <Button onClick={back} size="sm" variant="ghost">
          {text.back}
        </Button>
        <h1 className={styles.title}>{text.titleByAddress}</h1>
      </header>

      <RadioGroup
        label={text.methodLabel}
        onChange={(value) => {
          setMethod(value);
          setOutcome(null);
        }}
        options={PAIR_METHODS.map((value) => ({
          value,
          title: text.methods[value],
          disabled: locked,
        }))}
        value={method}
      />

      <div className={styles.form}>
        {method === 'address' ? (
          <>
            <div className={styles.addressRow}>
              <label className={styles.field}>
                <span className={styles.label}>{text.addressLabel}</span>
                <input
                  autoComplete="off"
                  className={styles.input}
                  disabled={locked}
                  onChange={(e) => setAddr(e.target.value)}
                  placeholder={text.addressPlaceholder}
                  spellCheck={false}
                  value={addr}
                />
              </label>
              <label className={[styles.field, styles.port].join(' ')}>
                <span className={styles.label}>{text.portLabel}</span>
                <input
                  aria-invalid={!portValid(port)}
                  className={styles.input}
                  disabled={locked}
                  inputMode="numeric"
                  onChange={(e) => setPort(e.target.value)}
                  placeholder={String(RSP_DEFAULT_PORT)}
                  value={port}
                />
              </label>
            </div>
            <div className={styles.field}>
              <label className={styles.field}>
                <span className={styles.label}>{text.codeLabel}</span>
                <input
                  aria-describedby={`${hintId}-code`}
                  autoComplete="one-time-code"
                  className={[styles.input, styles.code].join(' ')}
                  disabled={locked}
                  inputMode="numeric"
                  maxLength={6}
                  onChange={(e) => setCode(codeDigits(e.target.value))}
                  value={code}
                />
              </label>
              <span className={styles.hint} id={`${hintId}-code`}>
                {text.codeHint}
              </span>
            </div>
          </>
        ) : null}

        {method === 'file' ? (
          <div className={styles.file}>
            <div>
              <Button disabled={locked} onClick={() => void pickFile()} variant="secondary">
                {text.pickFile}
              </Button>
            </div>
            {file !== null ? (
              <KeyValueList
                items={[
                  { key: text.fileHost, value: file.hostName },
                  { key: text.fileExpires, value: formatTime(file.expiresAt) },
                ]}
              />
            ) : null}
            {file?.expired === true ? (
              <Banner role="status" tone="info" title={text.fileExpired} />
            ) : null}
            {fileError !== null ? <Banner role="alert" tone="info" title={fileError} /> : null}
          </div>
        ) : null}
      </div>

      {busy ? (
        <p className={styles.progress} role="status">
          {text.progress[step ?? 'dialing']}
        </p>
      ) : null}
      {outcome !== null ? (
        <Banner
          role={outcome.status === 'mitm' ? 'alert' : 'status'}
          title={outcomeText(outcome)}
          tone={outcomeTone(outcome)}
        />
      ) : null}

      <div className={styles.actions}>
        {busy ? (
          <Button onClick={cancel} variant="secondary">
            {text.cancel}
          </Button>
        ) : null}
        <Button disabled={!ready || locked} onClick={submit}>
          {text.start}
        </Button>
      </div>
    </section>
  );
}
