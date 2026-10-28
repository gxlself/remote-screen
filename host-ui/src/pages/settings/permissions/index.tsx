import { zh } from '../../../i18n';
import { useIpcQuery } from '../../../ipc';
import { SYSTEM_SETTINGS_URLS, usePlatform } from '../../../platform';
import { Banner, Button, Tag } from '../../../components';

import styles from './permissions.module.css';

type PermissionRow = {
  key: 'screen_recording' | 'accessibility';
  name: string;
  description: string;
  url: string;
};

const PERMISSION_ROWS: readonly PermissionRow[] = [
  {
    key: 'screen_recording',
    name: zh.permissions.items.screen_recording.name,
    description: zh.permissions.items.screen_recording.description,
    url: SYSTEM_SETTINGS_URLS.screenRecording,
  },
  {
    key: 'accessibility',
    name: zh.permissions.items.accessibility.name,
    description: zh.permissions.items.accessibility.description,
    url: SYSTEM_SETTINGS_URLS.accessibility,
  },
];

const KNOWN_KEYS = new Set<PermissionRow['key']>(PERMISSION_ROWS.map((row) => row.key));

function isGranted(value: unknown): boolean {
  return value === true;
}

function permissionRecord(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

export default function PermissionsSection() {
  const platform = usePlatform();
  const status = useIpcQuery('status.get', {
    refetchOn: ['event.permissionsChanged'],
    pollMs: 3000,
  });
  const permissions = permissionRecord(status.data?.permissions);
  const extraPermissions = Object.entries(permissions).filter(
    ([key, value]) => !KNOWN_KEYS.has(key as PermissionRow['key']) && typeof value === 'boolean',
  );
  const allGranted = PERMISSION_ROWS.every((row) => isGranted(permissions[row.key]));

  return (
    <section className={styles.section} data-section="permissions">
      <h2 className={styles.title}>{zh.permissions.title}</h2>
      <p className={styles.description}>{zh.permissions.description}</p>

      <div className={styles.card}>
        {PERMISSION_ROWS.map((row) => {
          const granted = isGranted(permissions[row.key]);
          return (
            <div className={styles.row} data-permission={row.key} key={row.key}>
              <div className={styles.copy}>
                <div className={styles.name}>{row.name}</div>
                <div className={styles.rowDescription}>{row.description}</div>
              </div>
              <div className={styles.actions}>
                <Tag tone={granted ? 'good' : 'bad'}>
                  {granted ? zh.permissions.granted : zh.permissions.notGranted}
                </Tag>
                {granted ? null : (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => void platform.openUrl(row.url)}
                  >
                    {zh.permissions.openSystemSettings}
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {extraPermissions.length > 0 ? (
        <ul className={styles.extraList}>
          {extraPermissions.map(([key, value]) => (
            <li key={key}>
              {key}：{value ? zh.permissions.granted : zh.permissions.notGranted}
            </li>
          ))}
        </ul>
      ) : null}

      {allGranted ? (
        <Banner tone="good">{zh.permissions.allGranted}</Banner>
      ) : (
        <Banner tone="info">{zh.permissions.needsRestart}</Banner>
      )}

      <div className={styles.footer}>
        <Button variant="ghost" onClick={() => void platform.showWindow('setup', 'setup')}>
          {zh.permissions.rerunSetup}
        </Button>
      </div>
    </section>
  );
}
