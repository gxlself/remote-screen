import { useEffect, useState } from 'react';

import { AppGlyph, Banner, Divider, StatusDot, Switch } from '../../components';
import { zh } from '../../i18n';
import { useIpc, useIpcConnected, useIpcQuery } from '../../ipc';
import { useNow } from '../../lib';
import { usePlatform } from '../../platform';

import { MenuRow } from './MenuRow';
import { ControllerCard, ViewerCard } from './SessionCard';
import styles from './Menu.module.css';
import type { PageProps } from '../../app/router';
import type { HostSettings, SessionSummary } from '../../ipc';

type AccessBooleanKey = 'allow_remote_control' | 'attended_mode' | 'lock_on_end';

interface SettingError {
  key: AccessBooleanKey;
  message: string;
}

const ACCESS_SWITCHES: readonly {
  key: AccessBooleanKey;
  labelId: string;
  label: string;
}[] = [
  {
    key: 'allow_remote_control',
    labelId: 'menu-allow-remote-control',
    label: zh.menu.access.allowRemoteControl,
  },
  {
    key: 'attended_mode',
    labelId: 'menu-attended-mode',
    label: zh.menu.access.attendedMode,
  },
  {
    key: 'lock_on_end',
    labelId: 'menu-lock-on-end',
    label: zh.menu.access.lockOnEnd,
  },
];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function orderedSessions(sessions: readonly SessionSummary[]): SessionSummary[] {
  return [
    ...sessions.filter((session) => session.role === 'controller'),
    ...sessions
      .filter((session) => session.role === 'viewer')
      .sort((left, right) => left.connected_at_us - right.connected_at_us),
  ];
}

export default function MenuPage(_props: PageProps) {
  const ipc = useIpc();
  const platform = usePlatform();
  const connected = useIpcConnected();
  const now = useNow();
  const [settingError, setSettingError] = useState<SettingError | null>(null);

  const statusQuery = useIpcQuery('status.get', {
    refetchOn: ['event.sessionsChanged', 'event.permissionsChanged'],
  });
  const sessionsQuery = useIpcQuery('sessions.list', {
    refetchOn: ['event.sessionsChanged'],
  });
  const settingsQuery = useIpcQuery('settings.get');
  const devicesQuery = useIpcQuery('devices.list', {
    refetchOn: ['event.pairingResult'],
  });
  const updateQuery = useIpcQuery('update.status', {
    refetchOn: ['event.updateState'],
  });

  const status = statusQuery.data;
  const settings = settingsQuery.data;
  const isWindows = platform.os === 'windows';
  const sessions = orderedSessions(sessionsQuery.data?.sessions ?? []);
  const sessionCount = sessionsQuery.data?.sessions.length ?? 0;
  const devicesCount = devicesQuery.data?.devices.length ?? 0;
  const updateVersion = updateQuery.data?.version ?? null;
  const online = status?.online === true && status.listening === true;
  const address = status?.tailnet_addresses[0];
  const permissions = status?.permissions;
  const permissionsGranted =
    permissions?.screen_recording === true && permissions.accessibility === true;

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        void platform.hideWindow('menu');
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [platform]);

  async function updateAccess(key: AccessBooleanKey, next: boolean): Promise<void> {
    const previous = settingsQuery.data;
    if (previous === undefined) {
      return;
    }

    setSettingError(null);
    settingsQuery.mutate({
      ...previous,
      access: { ...previous.access, [key]: next },
    });

    try {
      const saved = await ipc.call('settings.set', {
        access: { [key]: next } as Partial<HostSettings['access']>,
      });
      settingsQuery.mutate(saved);
    } catch (caught) {
      settingsQuery.mutate(previous);
      setSettingError({ key, message: errorMessage(caught) });
    }
  }

  async function openWindow(label: 'pair' | 'settings' | 'setup' | 'update', route: string) {
    await platform.showWindow(label, route);
    await platform.hideWindow('menu');
  }

  function disconnectSession(sessionId: string): void {
    void ipc.call('sessions.disconnect', { session_id: sessionId }).catch(() => undefined);
  }

  function setViewOnly(sessionId: string): void {
    void ipc.call('sessions.setViewOnly', { session_id: sessionId }).catch(() => undefined);
  }

  async function disconnectAll(): Promise<void> {
    const ids = sessions.map((session) => session.session_id);
    for (const sessionId of ids) {
      try {
        await ipc.call('sessions.disconnect', { session_id: sessionId });
      } catch {
        return;
      }
    }
  }

  return (
    <main className={styles.menu} data-page="menu">
      <header className={styles.header}>
        <AppGlyph size={32} />
        <div className={styles.identity}>
          <div className={styles.titleRow}>
            <span className={styles.title}>{zh.menu.appName}</span>
            {status === undefined ? null : <span className={styles.version}>{status.version}</span>}
          </div>
          <div className={styles.statusLine}>
            <StatusDot tone={online ? 'good' : 'bad'} />
            <span>{online ? zh.menu.online : zh.menu.offline}</span>
            {address === undefined ? null : (
              <>
                <span aria-hidden="true">{zh.menu.statusSeparator}</span>
                <span className={styles.address}>{address}</span>
              </>
            )}
          </div>
        </div>
      </header>

      {connected ? null : (
        <div className={styles.bannerWrap}>
          <Banner tone="bad" title={zh.common.agentDown}>
            {zh.common.agentDownHint}
          </Banner>
        </div>
      )}

      {sessionCount === 0 ? null : (
        <section className={styles.sessionBlock}>
          {sessionCount >= 2 ? (
            <div className={styles.sessionSummary}>
              <span className={styles.summaryTitle}>{zh.menu.connecting(sessionCount)}</span>
              <span className={styles.summaryNote}>{zh.menu.onlyOneController}</span>
            </div>
          ) : null}

          {sessions.map((session) =>
            session.role === 'controller' ? (
              <ControllerCard
                disabled={!connected}
                key={session.session_id}
                now={now}
                onDisconnect={disconnectSession}
                onViewOnly={setViewOnly}
                session={session}
              />
            ) : (
              <ViewerCard
                disabled={!connected}
                key={session.session_id}
                now={now}
                onDisconnect={disconnectSession}
                session={session}
              />
            ),
          )}

          {sessionCount >= 2 ? (
            <div className={styles.multiHint}>
              <p>{zh.menu.multiSessionHintFirst}</p>
              <p>{zh.menu.multiSessionHintSecond}</p>
            </div>
          ) : null}
        </section>
      )}

      <section className={styles.switchSection}>
        {ACCESS_SWITCHES.map(({ key, labelId, label }) => {
          const checked = settings?.access[key] ?? false;
          const disabled = !connected || settings === undefined;

          return (
            <div className={styles.switchGroup} key={key}>
              <div className={styles.switchRow}>
                <span className={styles.switchLabel} id={labelId}>
                  {label}
                </span>
                <Switch
                  checked={checked}
                  disabled={disabled}
                  labelledBy={labelId}
                  onChange={(next) => {
                    void updateAccess(key, next);
                  }}
                />
              </div>
              {settingError?.key === key ? (
                <div className={styles.settingError} role="alert">
                  {zh.menu.settingsSaveFailed(settingError.message)}
                </div>
              ) : null}
            </div>
          );
        })}
        <p className={styles.switchHint}>{zh.menu.access.screenBorderHint}</p>
        {isWindows ? (
          <div className={styles.serviceInfo}>
            <div className={styles.serviceInfoTitle}>{zh.menu.windows.serviceInfo}</div>
            <div className={styles.serviceInfoHint}>{zh.menu.windows.serviceInfoHint}</div>
          </div>
        ) : null}
      </section>

      <nav className={styles.entries}>
        <MenuRow
          disabled={!connected}
          onClick={() => {
            void openWindow('pair', 'pair');
          }}
        >
          {zh.menu.pairEntry}
        </MenuRow>

        {isWindows ? (
          <MenuRow
            disabled={!connected}
            onClick={() => {
              void openWindow('settings', 'settings/general');
            }}
          >
            {zh.menu.windows.settingsEntry}
          </MenuRow>
        ) : (
          <>
            <MenuRow
              disabled={!connected}
              onClick={() => {
                void openWindow(
                  permissionsGranted ? 'settings' : 'setup',
                  permissionsGranted ? 'settings/permissions' : 'setup',
                );
              }}
              trailing={
                status === undefined ? undefined : (
                  <span className={permissionsGranted ? styles.trailingGood : styles.trailingBad}>
                    {permissionsGranted ? zh.menu.permissionsGranted : zh.menu.permissionsMissing}
                  </span>
                )
              }
            >
              {zh.menu.permissionsEntry}
            </MenuRow>

            <MenuRow
              disabled={!connected}
              onClick={() => {
                void openWindow('settings', 'settings/devices');
              }}
              trailing={
                devicesQuery.data === undefined ? undefined : zh.menu.devicesCount(devicesCount)
              }
            >
              {zh.menu.devicesEntry}
            </MenuRow>

            <MenuRow
              disabled={!connected}
              onClick={() => {
                void openWindow('settings', 'settings/history');
              }}
            >
              {zh.menu.historyEntry}
            </MenuRow>
          </>
        )}

        {updateVersion === null ? null : (
          <MenuRow
            disabled={!connected}
            onClick={() => {
              void openWindow('update', 'update');
            }}
            tone="accent"
            trailing={
              isWindows ? (
                <span className={styles.updateAction}>{zh.menu.windows.updateAction}</span>
              ) : undefined
            }
          >
            {isWindows
              ? zh.menu.windows.updateAvailable(updateVersion)
              : zh.menu.updateAvailable(updateVersion)}
          </MenuRow>
        )}

        {sessionCount >= 2 ? (
          <MenuRow
            disabled={!connected}
            onClick={() => {
              void disconnectAll();
            }}
            tone="danger"
          >
            {zh.menu.disconnectAll}
          </MenuRow>
        ) : null}
      </nav>

      <Divider />

      <nav className={styles.entries}>
        <MenuRow
          disabled={!connected}
          onClick={() => {
            void platform.quit();
          }}
          trailing={
            isWindows ? undefined : <span className={styles.shortcut}>{zh.menu.quitShortcut}</span>
          }
        >
          {zh.menu.quit}
        </MenuRow>
      </nav>
    </main>
  );
}
