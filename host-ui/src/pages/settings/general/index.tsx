import { useCallback, useState, type ReactNode } from 'react';

import { Banner, Button, Switch } from '../../../components';
import { zh } from '../../../i18n';
import { useIpc, useIpcQuery } from '../../../ipc';

import type { DeepPartial, HostSettings } from '../../../ipc';

import styles from '../Settings.module.css';

const IDLE_TIMEOUTS: readonly number[] = [0, 15, 30, 60, 120];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function mergeSettings(current: HostSettings, patch: DeepPartial<HostSettings>): HostSettings {
  return {
    access: { ...current.access, ...patch.access },
    intranet: { ...current.intranet, ...patch.intranet },
    clipboard: { ...current.clipboard, ...patch.clipboard },
    files: { ...current.files, ...patch.files },
    video: { ...current.video, ...patch.video },
    update: { ...current.update, ...patch.update },
    log: { ...current.log, ...patch.log },
  };
}

function SettingRow({
  title,
  description,
  htmlFor,
  control,
}: {
  title: string;
  description?: string;
  htmlFor?: string;
  control: ReactNode;
}) {
  return (
    <div className={styles.row}>
      <div className={styles.rowCopy}>
        {htmlFor === undefined ? (
          <div className={styles.rowLabel}>{title}</div>
        ) : (
          <label className={styles.rowLabel} htmlFor={htmlFor}>
            {title}
          </label>
        )}
        {description === undefined ? null : <div className={styles.rowHint}>{description}</div>}
      </div>
      <div className={styles.rowControl}>{control}</div>
    </div>
  );
}

export default function GeneralSection() {
  const ipc = useIpc();
  const { data, mutate } = useIpcQuery('settings.get');
  const [saveError, setSaveError] = useState<string | null>(null);
  const [accountDraft, setAccountDraft] = useState('');
  const [accountError, setAccountError] = useState<string | null>(null);

  const copy = zh.settings.general;

  const save = useCallback(
    async (current: HostSettings, patch: DeepPartial<HostSettings>): Promise<boolean> => {
      setSaveError(null);
      mutate(mergeSettings(current, patch));

      try {
        const next = await ipc.call('settings.set', patch);
        mutate(next);
        return true;
      } catch (error) {
        mutate(current);
        setSaveError(errorMessage(error));
        return false;
      }
    },
    [ipc, mutate],
  );

  const addAccount = async (): Promise<void> => {
    if (data === undefined) {
      return;
    }

    const account = accountDraft.trim();
    if (account.length === 0) {
      setAccountError(copy.access.emptyAccount);
      return;
    }
    if (/\s/.test(account)) {
      setAccountError(copy.access.accountHasSpace);
      return;
    }
    if (data.access.allowed_users.includes(account)) {
      setAccountError(copy.access.accountExists);
      return;
    }

    setAccountError(null);
    const saved = await save(data, {
      access: { allowed_users: [...data.access.allowed_users, account] },
    });
    if (saved) {
      setAccountDraft('');
    }
  };

  const removeAccount = (account: string): void => {
    if (data === undefined) {
      return;
    }
    void save(data, {
      access: { allowed_users: data.access.allowed_users.filter((item) => item !== account) },
    });
  };

  const idleTimeout = data?.access.idle_timeout_min;
  const idleValues = [...IDLE_TIMEOUTS];
  if (idleTimeout !== undefined && !idleValues.includes(idleTimeout)) {
    idleValues.push(idleTimeout);
  }

  return (
    <section className={styles.section} data-section="general">
      <h2 className={styles.sectionTitle}>{copy.title}</h2>

      {saveError === null ? null : (
        <Banner role="alert" tone="bad">{`${copy.saveFailed}${saveError}`}</Banner>
      )}

      {data === undefined ? null : (
        <>
          <div className={styles.group}>
            <h3 className={styles.groupTitle}>{copy.groups.access}</h3>
            <div className={styles.rows}>
              <SettingRow
                control={
                  <Switch
                    checked={data.access.allow_remote_control}
                    label={copy.access.allowRemoteControl}
                    onChange={(next) => void save(data, { access: { allow_remote_control: next } })}
                  />
                }
                description={copy.access.allowRemoteControlHint}
                title={copy.access.allowRemoteControl}
              />
              <SettingRow
                control={
                  <Switch
                    checked={data.access.attended_mode}
                    label={copy.access.attendedMode}
                    onChange={(next) => void save(data, { access: { attended_mode: next } })}
                  />
                }
                description={copy.access.attendedModeHint}
                title={copy.access.attendedMode}
              />
              <SettingRow
                control={
                  <Switch
                    checked={data.access.pause_on_local_input}
                    label={copy.access.pauseOnLocalInput}
                    onChange={(next) => void save(data, { access: { pause_on_local_input: next } })}
                  />
                }
                title={copy.access.pauseOnLocalInput}
              />
              <SettingRow
                control={
                  <Switch
                    checked={data.access.lock_on_end}
                    label={copy.access.lockOnEnd}
                    onChange={(next) => void save(data, { access: { lock_on_end: next } })}
                  />
                }
                title={copy.access.lockOnEnd}
              />
              <SettingRow
                control={
                  <select
                    className={styles.select}
                    id="settings-idle-timeout"
                    onChange={(event) =>
                      void save(data, {
                        access: { idle_timeout_min: Number(event.target.value) },
                      })
                    }
                    value={String(data.access.idle_timeout_min)}
                  >
                    {idleValues.map((minutes) => (
                      <option key={minutes} value={String(minutes)}>
                        {minutes === 0 ? copy.access.idleNever : copy.access.idleMinutes(minutes)}
                      </option>
                    ))}
                  </select>
                }
                htmlFor="settings-idle-timeout"
                title={copy.access.idleTimeout}
              />

              <div className={styles.usersBlock}>
                <div className={styles.rowLabel}>{copy.access.allowedUsers}</div>
                {data.access.allowed_users.length === 0 ? (
                  <p className={styles.emptyUsers}>{copy.access.emptyUsers}</p>
                ) : (
                  <div className={styles.userList}>
                    {data.access.allowed_users.map((account) => (
                      <div className={styles.userRow} key={account}>
                        <span className={styles.account}>{account}</span>
                        <Button
                          aria-label={copy.access.removeAccount(account)}
                          onClick={() => removeAccount(account)}
                          size="sm"
                          variant="ghost"
                        >
                          {copy.access.remove}
                        </Button>
                      </div>
                    ))}
                  </div>
                )}
                <div className={styles.addRow}>
                  <input
                    aria-label={copy.access.accountPlaceholder}
                    className={styles.input}
                    onChange={(event) => {
                      setAccountDraft(event.target.value);
                      setAccountError(null);
                    }}
                    placeholder={copy.access.accountPlaceholder}
                    type="text"
                    value={accountDraft}
                  />
                  <Button onClick={() => void addAccount()} size="sm" variant="secondary">
                    {copy.access.add}
                  </Button>
                </div>
                {accountError === null ? null : (
                  <div className={styles.fieldError} role="alert">
                    {accountError}
                  </div>
                )}
              </div>
            </div>
          </div>

          <div className={styles.group}>
            <h3 className={styles.groupTitle}>{copy.groups.clipboard}</h3>
            <div className={styles.rows}>
              <SettingRow
                control={
                  <Switch
                    checked={data.clipboard.to_phone}
                    label={copy.clipboard.toPhone}
                    onChange={(next) => void save(data, { clipboard: { to_phone: next } })}
                  />
                }
                title={copy.clipboard.toPhone}
              />
              <SettingRow
                control={
                  <Switch
                    checked={data.clipboard.from_phone}
                    label={copy.clipboard.fromPhone}
                    onChange={(next) => void save(data, { clipboard: { from_phone: next } })}
                  />
                }
                title={copy.clipboard.fromPhone}
              />
            </div>
          </div>
        </>
      )}
    </section>
  );
}
