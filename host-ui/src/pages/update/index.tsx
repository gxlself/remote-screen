import { useState } from 'react';

import type { PageProps } from '../../app/router';
import {
  AppGlyph,
  Banner,
  Button,
  Checkbox,
  DeviceGlyph,
  Tag,
  WindowFrame,
} from '../../components';
import { zh } from '../../i18n';
import { useIpc, useIpcQuery } from '../../ipc';
import type { SessionSummary, UpdateChannel } from '../../ipc/types';
import { formatElapsed, useNow } from '../../lib';
import { usePlatform } from '../../platform';

import styles from './UpdatePage.module.css';

const copy = zh.update;

const BUSY_STATE_LABELS: Readonly<Record<string, string>> = {
  downloading: copy.status.downloading,
  verifying: copy.status.verifying,
  installing: copy.status.installing,
  restarting: copy.status.restarting,
};

type NoteLabel = 'fix' | 'feature';

interface NoteItem {
  label: NoteLabel | null;
  body: string;
}

function parseNotes(notes: string | null): NoteItem[] {
  if (notes === null) {
    return [];
  }

  return notes
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      if (line.startsWith(copy.notes.fixPrefix)) {
        return { label: 'fix', body: line.slice(copy.notes.fixPrefix.length).trim() };
      }
      if (line.startsWith(copy.notes.featurePrefix)) {
        return { label: 'feature', body: line.slice(copy.notes.featurePrefix.length).trim() };
      }
      return { label: null, body: line };
    });
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function platformOf(session: SessionSummary): string {
  return /ipad/i.test(session.device_name) ? 'ipados' : 'ios';
}

interface ChannelPickerProps {
  channel: UpdateChannel;
  disabled: boolean;
  onChange(channel: UpdateChannel): void;
}

function ChannelPicker({ channel, disabled, onChange }: ChannelPickerProps) {
  const channels: ReadonlyArray<{ value: UpdateChannel; label: string }> = [
    { value: 'stable', label: copy.channel.stable },
    { value: 'beta', label: copy.channel.beta },
  ];

  return (
    <div className={styles.channelSection}>
      <h3 className={styles.sectionTitle}>{copy.channel.title}</h3>
      <div aria-label={copy.channel.title} className={styles.segmented} role="radiogroup">
        {channels.map((option) => (
          <button
            aria-checked={channel === option.value}
            className={styles.channelOption}
            disabled={disabled}
            key={option.value}
            onClick={() => onChange(option.value)}
            role="radio"
            tabIndex={channel === option.value ? 0 : -1}
            type="button"
          >
            {option.label}
          </button>
        ))}
      </div>
      <p className={channel === 'beta' ? styles.channelHintBeta : styles.channelHint}>
        {channel === 'beta' ? copy.channel.betaHint : copy.channel.stableHint}
      </p>
    </div>
  );
}

export default function UpdatePage(_props: PageProps) {
  const platform = usePlatform();
  const ipc = useIpc();
  const now = useNow();
  const [confirmingInstall, setConfirmingInstall] = useState(false);
  const [installPending, setInstallPending] = useState(false);

  const updateQuery = useIpcQuery('update.status', { refetchOn: ['event.updateState'] });
  const sessionsQuery = useIpcQuery('sessions.list', {
    refetchOn: ['event.sessionsChanged'],
  });
  const settingsQuery = useIpcQuery('settings.get');
  const statusQuery = useIpcQuery('status.get');

  const hide = (): void => {
    void platform.hideWindow('update');
  };

  const update = updateQuery.data;
  const settings = settingsQuery.data;
  const sessions = sessionsQuery.data?.sessions ?? [];
  const currentVersion = statusQuery.data?.version ?? '—';
  const channel = settings?.update.channel ?? 'stable';
  const autoInstall = settings?.update.auto_install ?? true;
  const hasSessions = sessions.length > 0;
  const activeSession =
    sessions.find((session) => session.role === 'controller') ??
    sessions.find((session) => session.role === 'viewer') ??
    sessions[0];
  const busyLabel = update === undefined ? undefined : BUSY_STATE_LABELS[update.state];
  const controlsDisabled = busyLabel !== undefined || installPending;
  const confirmationActive = confirmingInstall && hasSessions;
  const showDisconnectAlert = hasSessions && (!autoInstall || confirmationActive);

  const handleAutoInstall = async (next: boolean): Promise<void> => {
    if (settings === undefined || controlsDisabled) {
      return;
    }

    const previous = settings;
    const optimistic = {
      ...previous,
      update: { ...previous.update, auto_install: next },
    };
    settingsQuery.mutate(optimistic);

    try {
      const saved = await ipc.call('settings.set', { update: { auto_install: next } });
      settingsQuery.mutate(saved);
    } catch (error) {
      settingsQuery.mutate(previous);
      window.alert(copy.settings.saveFailed(messageOf(error)));
    }
  };

  const handleChannelChange = async (next: UpdateChannel): Promise<void> => {
    if (next === channel || controlsDisabled) {
      return;
    }

    try {
      await ipc.call('update.setChannel', { channel: next });
      await settingsQuery.refetch();
    } catch (error) {
      window.alert(copy.channel.changeFailed(messageOf(error)));
    }
  };

  const handleInstallNow = async (): Promise<void> => {
    if (controlsDisabled) {
      return;
    }

    setInstallPending(true);
    try {
      await ipc.call('update.installNow');
    } catch (error) {
      window.alert(copy.install.failed(messageOf(error)));
    } finally {
      setInstallPending(false);
    }
  };

  if (update === undefined) {
    return (
      <main className={styles.page} data-page="update">
        <WindowFrame title={copy.title} titleId="update-window-title">
          <div className={styles.content}>
            <p className={styles.loading}>{zh.common.loading}</p>
          </div>
        </WindowFrame>
      </main>
    );
  }

  if (update.version === null) {
    return (
      <main className={styles.page} data-page="update">
        <WindowFrame
          footer={<Button onClick={hide}>{zh.common.close}</Button>}
          title={copy.title}
          titleId="update-window-title"
        >
          <div className={styles.content}>
            <div className={styles.latest}>
              <span className={styles.glyphFrame}>
                <AppGlyph size={56} />
              </span>
              <div className={styles.latestCopy}>
                <div className={styles.latestTitle}>{copy.latest.heading}</div>
                <div className={styles.latestVersion}>
                  {copy.latest.currentVersion}
                  <span className={styles.currentVersion}>{currentVersion}</span>
                </div>
              </div>
            </div>
            <div className={styles.latestChannel}>
              <ChannelPicker
                channel={channel}
                disabled={controlsDisabled}
                onChange={(next) => {
                  void handleChannelChange(next);
                }}
              />
            </div>
          </div>
        </WindowFrame>
      </main>
    );
  }

  const notes = parseNotes(update.notes);
  const downloaded = update.state === 'waiting_session_end' || update.state === 'idle';
  const channelLabel = channel === 'beta' ? copy.channel.beta : copy.channel.stable;
  const sessionTitle =
    activeSession === undefined
      ? ''
      : activeSession.role === 'controller'
        ? copy.session.controlling(activeSession.device_name)
        : copy.session.watching(activeSession.device_name);
  const sessionElapsed =
    activeSession === undefined ? '' : formatElapsed(now - activeSession.connected_at_us / 1000);
  const sessionNames = sessions.map((session) => session.device_name).join('、');

  let banner;
  if (busyLabel !== undefined) {
    banner = <Banner role="status" title={busyLabel} tone="info" />;
  } else if (update.state === 'failed') {
    banner = <Banner role="alert" tone="bad" title={copy.status.failed} />;
  } else if (activeSession !== undefined) {
    banner = (
      <Banner
        icon={<DeviceGlyph platform={platformOf(activeSession)} />}
        title={
          <div className={styles.controlledTitle}>
            <span>{sessionTitle}</span>
            <span className={styles.elapsed}>{sessionElapsed}</span>
          </div>
        }
        tone="controlled"
      >
        <span className={styles.sessionDetail}>
          {autoInstall ? copy.session.autoInstall : copy.session.manualInstall}
        </span>
      </Banner>
    );
  } else {
    banner = (
      <Banner role="status" title={copy.noSessions.title} tone="good">
        <span className={styles.sessionDetail}>{copy.noSessions.body}</span>
      </Banner>
    );
  }

  let secondaryAction;
  let primaryAction;
  if (!hasSessions) {
    secondaryAction = (
      <Button disabled={controlsDisabled} onClick={hide} variant="secondary">
        {copy.install.later}
      </Button>
    );
    primaryAction = (
      <Button disabled={controlsDisabled} onClick={() => void handleInstallNow()}>
        {copy.install.now}
      </Button>
    );
  } else if (confirmationActive) {
    secondaryAction = (
      <Button
        disabled={controlsDisabled}
        onClick={() => setConfirmingInstall(false)}
        variant="secondary"
      >
        {copy.install.cancel}
      </Button>
    );
    primaryAction = (
      <Button disabled={controlsDisabled} onClick={() => void handleInstallNow()}>
        {copy.install.disconnectAndInstall}
      </Button>
    );
  } else if (autoInstall) {
    secondaryAction = (
      <Button
        disabled={controlsDisabled}
        onClick={() => setConfirmingInstall(true)}
        variant="secondary"
      >
        {copy.install.now}
      </Button>
    );
    primaryAction = (
      <Button disabled={controlsDisabled} onClick={hide}>
        {copy.install.endAndInstall}
      </Button>
    );
  } else {
    secondaryAction = (
      <Button disabled={controlsDisabled} onClick={hide} variant="secondary">
        {copy.install.remindLater}
      </Button>
    );
    primaryAction = (
      <Button disabled={controlsDisabled} onClick={() => void handleInstallNow()}>
        {copy.install.disconnectAndInstall}
      </Button>
    );
  }

  return (
    <main className={styles.page} data-page="update">
      <WindowFrame
        footer={
          <div className={styles.footerBar}>
            <div className={styles.signatureHint}>
              <svg
                aria-hidden="true"
                fill="none"
                height="14"
                stroke="currentColor"
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth="1.5"
                viewBox="0 0 24 24"
                width="14"
              >
                <path d="M12 3 5.5 5.8v5.4c0 4.2 2.7 7.9 6.5 9.8 3.8-1.9 6.5-5.6 6.5-9.8V5.8L12 3Z" />
                <path d="m9 11.7 2 2 4-4" />
              </svg>
              <span>{copy.footer.signed}</span>
            </div>
            <div className={styles.footerActions}>
              {secondaryAction}
              {primaryAction}
            </div>
          </div>
        }
        title={copy.title}
        titleId="update-window-title"
      >
        <div className={styles.content}>
          <header className={styles.hero}>
            <span className={styles.glyphFrame}>
              <AppGlyph size={56} />
            </span>
            <div className={styles.heroCopy}>
              <h2 className={styles.heroTitle}>{copy.heading}</h2>
              <div className={styles.versions}>
                <span className={styles.currentVersion}>{currentVersion}</span>
                <span aria-hidden="true" className={styles.arrow}>
                  →
                </span>
                <span className={styles.nextVersion}>{update.version}</span>
                <Tag tone="neutral">{channelLabel}</Tag>
                {downloaded ? <Tag tone="good">{copy.downloaded}</Tag> : null}
              </div>
            </div>
          </header>

          {banner}

          <section aria-labelledby="update-notes-title" className={styles.notesSection}>
            <h3 className={styles.sectionTitle} id="update-notes-title">
              {copy.notes.title}
            </h3>
            {notes.length === 0 ? (
              <p className={styles.emptyNotes}>{copy.notes.empty}</p>
            ) : (
              <ul className={styles.notesList}>
                {notes.map((note, index) => (
                  <li className={styles.noteRow} key={`${index}-${note.body}`}>
                    {note.label === null ? null : (
                      <span
                        className={[
                          styles.noteLabel,
                          note.label === 'fix' ? styles.noteLabelFix : styles.noteLabelFeature,
                        ].join(' ')}
                      >
                        {note.label === 'fix' ? copy.notes.fixLabel : copy.notes.featureLabel}
                      </span>
                    )}
                    <span className={styles.noteBody}>{note.body}</span>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <div className={styles.columns}>
            <section aria-labelledby="update-schedule-title">
              <h3 className={styles.sectionTitle} id="update-schedule-title">
                {copy.schedule.title}
              </h3>
              <Checkbox
                checked={autoInstall}
                description={copy.schedule.autoInstallHint}
                disabled={controlsDisabled}
                label={copy.schedule.autoInstall}
                onChange={(next) => {
                  void handleAutoInstall(next);
                }}
              />
            </section>
            <ChannelPicker
              channel={channel}
              disabled={controlsDisabled}
              onChange={(next) => {
                void handleChannelChange(next);
              }}
            />
          </div>

          {showDisconnectAlert ? (
            <div className={styles.disconnectAlert} role="alert">
              {copy.disconnectAlert(sessionNames)}
            </div>
          ) : null}
        </div>
      </WindowFrame>
    </main>
  );
}
