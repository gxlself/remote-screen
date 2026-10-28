import { useId, useState, type JSX } from 'react';

import {
  Banner,
  Button,
  Checkbox,
  DeviceGlyph,
  RadioGroup,
  type RadioGroupOption,
} from '../../../components';
import { zh } from '../../../i18n';
import type { DeviceSummary, Permission } from '../../../ipc';
import { formatWhen } from '../../../lib';
import { navigate } from '../../../app/router';

import styles from './Devices.module.css';

const PATH_ORDER = ['tailnet', 'lan', 'intranet'] as const;
type DevicePath = (typeof PATH_ORDER)[number];

const PERMISSION_OPTIONS: readonly RadioGroupOption<Permission>[] = [
  {
    value: 'view_only',
    title: zh.devices.permission.view_only.title,
    description: zh.devices.permission.view_only.description,
  },
  {
    value: 'control',
    title: zh.devices.permission.control.title,
    description: zh.devices.permission.control.description,
  },
  {
    value: 'unattended',
    title: zh.devices.permission.unattended.title,
    description: zh.devices.permission.unattended.description,
  },
];

interface DeviceDetailProps {
  device: DeviceSummary;
  intranetEnabled: boolean;
  now: number;
  onSave(changes: { permission: Permission; paths: string[] }): Promise<void>;
  onRevoke(): Promise<void>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function orderedPaths(paths: Iterable<string>): string[] {
  const selected = new Set(paths);
  return PATH_ORDER.filter((path) => selected.has(path));
}

export function DeviceDetail({
  device,
  intranetEnabled,
  now,
  onSave,
  onRevoke,
}: DeviceDetailProps): JSX.Element {
  const permissionId = useId();
  const pathsId = useId();
  const [saveError, setSaveError] = useState<string | null>(null);
  const [keepPathError, setKeepPathError] = useState(false);
  const [revokeConfirming, setRevokeConfirming] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState(false);

  const selectedPaths = new Set(device.paths);
  const lastConnected =
    device.last_connected_at_us === null
      ? zh.devices.notConnected
      : `${zh.devices.lastConnected} ${formatWhen(device.last_connected_at_us, now)}`;

  const save = async (changes: { permission: Permission; paths: string[] }): Promise<void> => {
    setSaveError(null);
    try {
      await onSave(changes);
    } catch (error) {
      setSaveError(errorMessage(error));
    }
  };

  const changePermission = (permission: Permission): void => {
    if (permission === device.permission) {
      return;
    }
    void save({ permission, paths: orderedPaths(device.paths) });
  };

  const togglePath = (path: DevicePath, next: boolean): void => {
    if (path === 'intranet' && !intranetEnabled) {
      return;
    }

    const nextPaths = new Set(device.paths);
    if (next) {
      nextPaths.add(path);
    } else {
      if (nextPaths.size <= 1) {
        setKeepPathError(true);
        return;
      }
      nextPaths.delete(path);
    }

    setKeepPathError(false);
    void save({ permission: device.permission, paths: orderedPaths(nextPaths) });
  };

  const revoke = async (): Promise<void> => {
    setRevokeError(null);
    setRevoking(true);
    try {
      await onRevoke();
    } catch (error) {
      setRevokeError(errorMessage(error));
      setRevoking(false);
    }
  };

  return (
    <section aria-label={zh.devices.detailLabel} className={styles.detail}>
      <header className={styles.detailHeader}>
        <DeviceGlyph platform="unknown" size={44} />
        <div className={styles.detailIdentity}>
          <h2 className={styles.detailName}>{device.name}</h2>
          <p className={styles.detailMeta}>
            <span className={styles.mono}>{device.device_id}</span>
            <span> · {lastConnected}</span>
          </p>
        </div>
      </header>

      {saveError === null ? null : (
        <div className={styles.detailBanner}>
          <Banner role="alert" tone="bad">
            {`${zh.devices.saveFailed}${saveError}`}
          </Banner>
        </div>
      )}

      <div className={styles.group}>
        <h3 className={styles.groupTitle} id={permissionId}>
          {zh.devices.permission.title}
        </h3>
        <RadioGroup
          boxed
          labelledBy={permissionId}
          onChange={changePermission}
          options={[...PERMISSION_OPTIONS]}
          value={device.permission}
        />
      </div>

      <div className={styles.group}>
        <h3 className={styles.groupTitle} id={pathsId}>
          {zh.devices.paths.title}
        </h3>
        <div aria-labelledby={pathsId} className={styles.pathList} role="group">
          <Checkbox
            checked={selectedPaths.has('tailnet')}
            description={zh.devices.paths.tailnet.description}
            label={zh.devices.paths.tailnet.title}
            onChange={(next) => togglePath('tailnet', next)}
          />
          <Checkbox
            checked={selectedPaths.has('lan')}
            description={zh.devices.paths.lan.description}
            label={zh.devices.paths.lan.title}
            onChange={(next) => togglePath('lan', next)}
          />
          <div className={styles.intranetRow}>
            <Checkbox
              checked={selectedPaths.has('intranet')}
              description={
                intranetEnabled
                  ? zh.devices.paths.intranet.descriptionEnabled
                  : zh.devices.paths.intranet.descriptionDisabled
              }
              disabled={!intranetEnabled}
              label={zh.devices.paths.intranet.title}
              onChange={(next) => togglePath('intranet', next)}
            />
            {intranetEnabled ? null : (
              <button
                className={styles.linkButton}
                onClick={() => navigate('settings/network')}
                type="button"
              >
                {zh.devices.paths.openNetwork}
              </button>
            )}
          </div>
        </div>
        {keepPathError ? (
          <p className={styles.pathError} role="alert">
            {zh.devices.paths.keepOnePath}
          </p>
        ) : null}
      </div>

      <div className={styles.revokeBlock}>
        {revokeError === null ? null : (
          <div className={styles.revokeBanner}>
            <Banner role="alert" tone="bad">
              {`${zh.devices.revoke.failed}${revokeError}`}
            </Banner>
          </div>
        )}

        {revokeConfirming ? (
          <div className={styles.revokeConfirm}>
            <span>{zh.devices.revoke.confirm(device.name)}</span>
            <Button disabled={revoking} onClick={() => void revoke()} variant="danger">
              {zh.devices.revoke.confirmAction}
            </Button>
            <Button disabled={revoking} onClick={() => setRevokeConfirming(false)} variant="ghost">
              {zh.devices.revoke.cancel}
            </Button>
          </div>
        ) : (
          <>
            <Button onClick={() => setRevokeConfirming(true)} variant="danger">
              {zh.devices.revoke.action}
            </Button>
            <p className={styles.revokeHint}>{zh.devices.revoke.hint(device.name)}</p>
          </>
        )}
      </div>
    </section>
  );
}
