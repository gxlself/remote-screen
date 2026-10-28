import { useCallback, useState, type ChangeEvent, type JSX } from 'react';

import {
  Banner,
  Button,
  RadioGroup,
  Switch,
  Tag,
  type RadioGroupOption,
  type TagTone,
} from '../../../components';
import { zh } from '../../../i18n';
import type {
  DeepPartial,
  HostSettings,
  PairFileInfo,
  PairFileState,
  Permission,
} from '../../../ipc';
import { IpcError, useIpc, useIpcQuery } from '../../../ipc';
import { formatDateTime } from '../../../lib';
import { usePlatform } from '../../../platform';

import styles from './pairing.module.css';

const MAX_LABEL_LENGTH = 64;

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

const STATE_TONES: Record<PairFileState, TagTone> = {
  active: 'good',
  used: 'neutral',
  failed: 'bad',
  revoked: 'neutral',
  expired: 'neutral',
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isUnsupported(error: unknown): boolean {
  return error instanceof IpcError && error.code === 'unsupported';
}

function defaultSecurity(settings: HostSettings): NonNullable<HostSettings['security']> {
  return {
    confirm_new_devices: true,
    password_pairing: false,
    password_pairing_permission: 'control',
    ...settings.security,
  };
}

function fileLabel(file: PairFileInfo): string {
  const label = file.label?.trim() ?? '';
  return label.length > 0
    ? label
    : `${zh.pairingMethods.files.unknownLabel} ${file.file_id.slice(0, 6)}`;
}

function stateLabel(state: PairFileState): string {
  return zh.pairingMethods.state[state];
}

function whenLabel(us: number): string {
  return formatDateTime(us) || String(us);
}

function exportedDescription(file: PairFileInfo): string {
  if (file.state === 'used' && file.used_at !== null) {
    const device = file.used_by_device_id ?? '';
    const used = `${zh.pairingMethods.files.usedAt} ${whenLabel(file.used_at)}`;
    return device.length > 0 ? `${used} · ${zh.pairingMethods.files.usedBy} ${device}` : used;
  }
  return `${zh.pairingMethods.files.expiresAt} ${whenLabel(file.expires_at)}`;
}

export default function PairingSection(): JSX.Element {
  const ipc = useIpc();
  const platform = usePlatform();
  const settingsQuery = useIpcQuery('settings.get');
  const filesQuery = useIpcQuery('pairing.listFiles');
  const [saveError, setSaveError] = useState<string | null>(null);
  const [permission, setPermission] = useState<Permission>('control');
  const [label, setLabel] = useState('');
  const [exporting, setExporting] = useState(false);
  const [exportUnsupported, setExportUnsupported] = useState(false);
  const [exported, setExported] = useState<{
    file_name: string;
    path: string;
    expires_at: number;
  } | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  const copy = zh.pairingMethods;
  const security = settingsQuery.data === undefined ? null : defaultSecurity(settingsQuery.data);
  const files = filesQuery.data?.files ?? [];
  const filesUnsupported = filesQuery.error !== undefined && isUnsupported(filesQuery.error);
  const mutateSettings = settingsQuery.mutate;

  const save = useCallback(
    async (patch: DeepPartial<HostSettings>): Promise<void> => {
      const current = settingsQuery.data;
      if (current === undefined) {
        return;
      }

      setSaveError(null);
      const optimistic: HostSettings = {
        ...current,
        security: {
          ...defaultSecurity(current),
          ...(patch.security ?? {}),
        },
      };
      mutateSettings(optimistic);

      try {
        const next = await ipc.call('settings.set', patch);
        mutateSettings(next);
      } catch (error) {
        mutateSettings(current);
        setSaveError(errorMessage(error));
      }
    },
    [ipc, mutateSettings, settingsQuery.data],
  );

  const changePasswordPairing = (next: boolean): void => {
    if (settingsQuery.data === undefined) {
      return;
    }
    void save({ security: { password_pairing: next } });
  };

  const changePasswordPermission = (next: Permission): void => {
    if (settingsQuery.data === undefined || security === null) {
      return;
    }
    if (security.password_pairing_permission === next) {
      return;
    }
    void save({ security: { password_pairing_permission: next } });
  };

  const exportFile = async (): Promise<void> => {
    const trimmed = label.trim();
    if (trimmed.length > MAX_LABEL_LENGTH) {
      return;
    }

    setExporting(true);
    setExported(null);
    setExportError(null);
    setExportUnsupported(false);

    try {
      const result = await ipc.call('pairing.exportFile', {
        permission,
        ...(trimmed.length > 0 ? { label: trimmed } : {}),
      });
      setExported(result);
      void filesQuery.refetch();
    } catch (error) {
      if (error instanceof IpcError && error.code === 'unsupported') {
        setExportUnsupported(true);
      } else if (error instanceof IpcError && error.code === 'too_many_files') {
        setExportError(copy.file.errors.tooMany);
      } else if (error instanceof IpcError && error.code === 'io') {
        setExportError(`${copy.file.errors.io}${errorMessage(error)}`);
      } else {
        setExportError(`${copy.file.errors.generic}${errorMessage(error)}`);
      }
    } finally {
      setExporting(false);
    }
  };

  const revokeFile = async (fileId: string): Promise<void> => {
    setRevokeError(null);
    setRevokingId(fileId);
    try {
      await ipc.call('pairing.revokeFile', { file_id: fileId });
      setConfirmingId(null);
      await filesQuery.refetch();
    } catch (error) {
      setRevokeError(`${copy.files.revokeFailed}${errorMessage(error)}`);
    } finally {
      setRevokingId(null);
    }
  };

  const onLabelChange = (event: ChangeEvent<HTMLInputElement>): void => {
    setLabel(event.target.value);
    setExportError(null);
  };

  const labelTooLong = label.trim().length > MAX_LABEL_LENGTH;

  return (
    <section className={styles.section} data-section="pairing">
      <h2 className={styles.title}>{copy.title}</h2>
      <p className={styles.intro}>{copy.intro}</p>

      {saveError === null ? null : (
        <Banner role="alert" tone="bad">{`${copy.errors.save}${saveError}`}</Banner>
      )}

      {settingsQuery.data === undefined ? null : security === null ? (
        <Banner tone="info">{copy.errors.notAvailable}</Banner>
      ) : (
        <div className={styles.group} data-pairing-block="password">
          <h3 className={styles.groupTitle}>{copy.password.title}</h3>
          <div className={styles.switchRow}>
            <div className={styles.switchCopy}>
              <div className={styles.switchLabel}>{copy.password.switchLabel}</div>
              <div className={styles.switchHint}>{copy.password.hint}</div>
            </div>
            <Switch
              checked={security.password_pairing}
              label={copy.password.switchLabel}
              onChange={changePasswordPairing}
            />
          </div>

          {security.password_pairing ? (
            <div className={styles.permissionBlock}>
              <h4 className={styles.permissionTitle}>{copy.password.permissionTitle}</h4>
              <RadioGroup
                boxed
                label={copy.password.permissionTitle}
                onChange={changePasswordPermission}
                options={[...PERMISSION_OPTIONS]}
                value={security.password_pairing_permission}
              />
            </div>
          ) : null}
        </div>
      )}

      <div className={styles.group} data-pairing-block="file">
        <h3 className={styles.groupTitle}>{copy.file.title}</h3>
        <p className={styles.groupHint}>{copy.file.hint}</p>

        {exportUnsupported ? (
          <div className={styles.unsupported}>{copy.file.unsupported}</div>
        ) : (
          <div className={styles.exportCard}>
            <h4 className={styles.permissionTitle}>{copy.file.permissionTitle}</h4>
            <RadioGroup
              boxed
              label={copy.file.permissionTitle}
              onChange={(next) => setPermission(next)}
              options={[...PERMISSION_OPTIONS]}
              value={permission}
            />

            <div className={styles.labelRow}>
              <label className={styles.label} htmlFor="pairing-file-label">
                {copy.file.labelTitle}
              </label>
              <span
                className={labelTooLong ? `${styles.counter} ${styles.counterBad}` : styles.counter}
              >
                {copy.file.labelLength(label.length)}
              </span>
            </div>
            <input
              aria-describedby={labelTooLong ? 'pairing-file-label-error' : undefined}
              aria-invalid={labelTooLong}
              className={styles.input}
              id="pairing-file-label"
              onChange={onLabelChange}
              placeholder={copy.file.labelPlaceholder}
              type="text"
              value={label}
            />
            {labelTooLong ? (
              <p className={styles.fieldError} id="pairing-file-label-error" role="alert">
                {copy.file.labelTooLong}
              </p>
            ) : null}

            <div className={styles.exportActions}>
              <Button disabled={exporting || labelTooLong} onClick={() => void exportFile()}>
                {exporting ? copy.file.exporting : copy.file.export}
              </Button>
            </div>

            {exportError === null ? null : (
              <div className={styles.error}>
                <Banner role="alert" tone="bad">
                  {exportError}
                </Banner>
              </div>
            )}

            {exported === null ? null : (
              <div className={styles.exported}>
                <div className={styles.exportedTitle}>{copy.file.exportedTitle}</div>
                <div className={styles.detail}>
                  {`${copy.file.fileName}：`}
                  <span className={styles.mono}>{exported.file_name}</span>
                </div>
                <div className={styles.detail}>
                  {`${copy.file.path}：`}
                  <span className={styles.mono}>{exported.path}</span>
                </div>
                <div className={styles.detail}>
                  {`${copy.file.expiresAt}：${whenLabel(exported.expires_at)}`}
                </div>
                <div className={styles.exportActions}>
                  <Button
                    onClick={() => void platform.revealPath(exported.path)}
                    size="sm"
                    variant="ghost"
                  >
                    {copy.file.reveal}
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      <div className={styles.group} data-pairing-block="files">
        <h3 className={styles.groupTitle}>{copy.files.title}</h3>

        {revokeError === null ? null : (
          <div className={styles.error}>
            <Banner role="alert" tone="bad">
              {revokeError}
            </Banner>
          </div>
        )}

        {filesUnsupported ? (
          <div className={styles.unsupported}>{copy.file.unsupported}</div>
        ) : filesQuery.loading ? (
          <div className={styles.empty}>{copy.files.loading}</div>
        ) : filesQuery.error !== undefined ? (
          <Banner role="alert" tone="bad">
            {`${copy.files.loadFailed}${errorMessage(filesQuery.error)}`}
          </Banner>
        ) : files.length === 0 ? (
          <div className={styles.fileList}>
            <div className={styles.empty}>{copy.files.empty}</div>
          </div>
        ) : (
          <div className={styles.fileList} role="list">
            {files.map((file) => (
              <div className={styles.fileRow} key={file.file_id} role="listitem">
                <div className={styles.fileCopy}>
                  <div className={styles.fileName}>{fileLabel(file)}</div>
                  <div className={styles.fileMeta}>
                    <Tag tone={STATE_TONES[file.state]}>{stateLabel(file.state)}</Tag>
                    <span>
                      {`${copy.files.permission}：${zh.devices.permissionTag[file.permission]}`}
                    </span>
                    <span>{exportedDescription(file)}</span>
                  </div>
                </div>
                {file.state === 'active' ? (
                  confirmingId === file.file_id ? (
                    <div className={styles.fileActions}>
                      <span>{copy.files.revokeConfirm}</span>
                      <Button
                        disabled={revokingId !== null}
                        onClick={() => void revokeFile(file.file_id)}
                        size="sm"
                        variant="danger"
                      >
                        {copy.files.revoke}
                      </Button>
                      <Button
                        disabled={revokingId !== null}
                        onClick={() => setConfirmingId(null)}
                        size="sm"
                        variant="ghost"
                      >
                        {zh.common.cancel}
                      </Button>
                    </div>
                  ) : (
                    <div className={styles.fileActions}>
                      <Button
                        onClick={() => setConfirmingId(file.file_id)}
                        size="sm"
                        variant="ghost"
                      >
                        {copy.files.revoke}
                      </Button>
                    </div>
                  )
                ) : null}
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
