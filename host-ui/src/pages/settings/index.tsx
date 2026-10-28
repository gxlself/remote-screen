import { AppGlyph, Banner } from '../../components';
import { zh } from '../../i18n';
import { useIpcConnected, useIpcQuery } from '../../ipc';
import { firstIPv4 } from '../../lib';

import { navigate, type PageProps } from '../../app/router';
import { SETTINGS_SECTIONS, type SectionId } from './sections';

import styles from './Settings.module.css';

function isSectionId(value: string | undefined): value is SectionId {
  return value !== undefined && SETTINGS_SECTIONS.some((section) => section.id === value);
}

export default function SettingsPage({ route }: PageProps) {
  const connected = useIpcConnected();
  const status = useIpcQuery('status.get');
  const devices = useIpcQuery('devices.list', { refetchOn: ['event.pairingResult'] });

  const activeId: SectionId = isSectionId(route.section) ? route.section : 'general';
  const activeSection =
    SETTINGS_SECTIONS.find((section) => section.id === activeId) ??
    SETTINGS_SECTIONS.find((section) => section.id === 'general');
  const ActiveSection = activeSection?.component;

  const version = status.data?.version ?? '';
  const tailnetAddress = firstIPv4(status.data?.tailnet_addresses);
  const deviceCount = devices.data?.devices.length ?? 0;

  return (
    <main className={styles.shell} data-page="settings">
      <aside className={styles.sidebar}>
        <div className={styles.dragRegion} data-tauri-drag-region />

        <header className={styles.brand}>
          <AppGlyph size={28} />
          <div className={styles.brandCopy}>
            <div className={styles.brandTitle}>{zh.settings.shell.title}</div>
            <div className={styles.version}>{version}</div>
          </div>
        </header>

        <nav aria-label={zh.settings.shell.navLabel} className={styles.nav}>
          {SETTINGS_SECTIONS.map((section) => {
            const current = section.id === activeId;
            const count = section.id === 'devices' ? deviceCount : 0;
            const label = zh.common.settingsNav[section.id];

            return (
              <button
                aria-current={current ? 'page' : undefined}
                className={[styles.navButton, current ? styles.navCurrent : undefined]
                  .filter(Boolean)
                  .join(' ')}
                key={section.id}
                onClick={() => navigate(`settings/${section.id}`)}
                type="button"
              >
                <span>{label}</span>
                {count > 0 ? (
                  <span aria-hidden="true" className={styles.navCount}>
                    {count}
                  </span>
                ) : null}
              </button>
            );
          })}
        </nav>

        <footer className={styles.localInfo}>
          <div>{zh.settings.shell.thisMac}</div>
          <div className={styles.localAddress}>
            {tailnetAddress ?? zh.settings.shell.tailnetMissing}
          </div>
        </footer>
      </aside>

      <div className={styles.content}>
        {!connected ? (
          <div className={styles.contentBanner}>
            <Banner title={zh.common.agentDown} tone="bad">
              {zh.common.agentDownHint}
            </Banner>
          </div>
        ) : null}
        {ActiveSection !== undefined ? <ActiveSection /> : null}
      </div>
    </main>
  );
}
