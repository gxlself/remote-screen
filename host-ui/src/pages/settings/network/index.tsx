import { useEffect, useId, useRef, useState, type ReactNode } from 'react';

import { Banner, Button, Checkbox, StatusDot, Switch } from '../../../components';
import { zh } from '../../../i18n';
import { useIpc, useIpcQuery } from '../../../ipc';
import type { HostSettings } from '../../../ipc';
import { firstIPv4, isValidCidr } from '../../../lib';
import { usePlatform } from '../../../platform';

import styles from './network.module.css';

interface NicView {
  name: string;
  address: string | null;
  tailnet: boolean;
}

interface CopyFeedback {
  copied: boolean;
  run(action: () => Promise<void>): Promise<void>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseNics(value: unknown): NicView[] {
  if (!Array.isArray(value)) {
    return [];
  }

  const nics: NicView[] = [];
  for (const item of value) {
    if (!isRecord(item) || typeof item.name !== 'string' || item.name.trim().length === 0) {
      continue;
    }
    nics.push({
      name: item.name,
      address: firstIPv4(item.addresses),
      tailnet: item.tailnet === true,
    });
  }
  return nics;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function useCopyFeedback(): CopyFeedback {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
      }
    },
    [],
  );

  const run = async (action: () => Promise<void>): Promise<void> => {
    await action();
    setCopied(true);
    if (timer.current !== null) {
      clearTimeout(timer.current);
    }
    timer.current = setTimeout(() => {
      setCopied(false);
      timer.current = null;
    }, 2000);
  };

  return { copied, run };
}

function NetworkIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="20" viewBox="0 0 20 20" width="20">
      <circle cx="10" cy="10" r="7.25" stroke="currentColor" strokeWidth="1.3" />
      <path
        d="M3.1 7.3h13.8M3.1 12.7h13.8M10 2.75c2 2 3 4.4 3 7.25s-1 5.25-3 7.25c-2-2-3-4.4-3-7.25s1-5.25 3-7.25Z"
        stroke="currentColor"
        strokeWidth="1.1"
      />
    </svg>
  );
}

function WarningIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="13" viewBox="0 0 14 14" width="13">
      <path
        d="M7 1.3 12.7 11H1.3L7 1.3Z"
        stroke="currentColor"
        strokeLinejoin="round"
        strokeWidth="1.15"
      />
      <path d="M7 5v2.7" stroke="currentColor" strokeLinecap="round" strokeWidth="1.15" />
      <circle cx="7" cy="9.55" fill="currentColor" r="0.65" />
    </svg>
  );
}

function GoodIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="16" viewBox="0 0 16 16" width="16">
      <circle cx="8" cy="8" r="6.5" stroke="currentColor" strokeWidth="1.2" />
      <path
        d="m4.8 8.1 2.1 2.1 4.3-4.4"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.35"
      />
    </svg>
  );
}

function RowCopy({
  title,
  description,
  children,
  divider = false,
}: {
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  divider?: boolean;
}) {
  return (
    <div
      className={[styles.row, divider ? styles.rowDivider : undefined].filter(Boolean).join(' ')}
    >
      <div className={styles.rowCopy}>
        <div className={styles.rowTitle}>{title}</div>
        {description !== undefined ? (
          <div className={styles.rowDescription}>{description}</div>
        ) : null}
      </div>
      {children !== undefined ? <div className={styles.rowTrailing}>{children}</div> : null}
    </div>
  );
}

export default function NetworkSection() {
  const ipc = useIpc();
  const platform = usePlatform();
  const settingsQuery = useIpcQuery('settings.get');
  const statusQuery = useIpcQuery('status.get');
  const latestSettings = useRef(settingsQuery.data);
  const settings = settingsQuery.data;
  useEffect(() => {
    if (settings !== undefined) {
      latestSettings.current = settings;
    }
  }, [settings]);
  const intranet = settings?.intranet;
  const enabled = intranet?.enabled === true;

  const nicsQuery = useIpcQuery('network.listNics', { enabled });
  const firewallQuery = useIpcQuery('network.firewallStatus', { enabled });

  const [saveError, setSaveError] = useState<string | null>(null);
  const [sourceInput, setSourceInput] = useState('');
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [itNoteError, setItNoteError] = useState<string | null>(null);
  const phoneCopy = useCopyFeedback();
  const itNoteCopy = useCopyFeedback();

  const uid = useId();
  const intranetLabelId = `${uid}-intranet`;
  const confirmEachId = `${uid}-confirm-each`;
  const allowPairingId = `${uid}-allow-pairing`;

  const tailnetAddress = statusQuery.data?.tailnet_addresses[0] ?? null;
  const fields = settings?.intranet;
  const selectedInterfaces = new Set(fields?.interfaces ?? []);
  const allowSources = fields?.allow_sources ?? [];
  const nics = parseNics(nicsQuery.data?.nics);
  const visibleNics = nics.filter((nic) => !nic.tailnet);
  const knownNames = new Set(visibleNics.map((nic) => nic.name));
  const missingNames = Array.from(new Set(fields?.interfaces ?? [])).filter(
    (name) => !knownNames.has(name),
  );
  const nicRows = [
    ...visibleNics.map((nic) => ({ name: nic.name, address: nic.address, missing: false })),
    ...missingNames.map((name) => ({ name, address: null, missing: true })),
  ];
  const nicOrder = nicRows.map((nic) => nic.name);

  let phoneAddress: string | null = null;
  for (const nic of visibleNics) {
    if (selectedInterfaces.has(nic.name) && nic.address !== null) {
      phoneAddress = nic.address;
      break;
    }
  }

  const alerts: string[] = [];
  if (enabled) {
    if ((fields?.interfaces.length ?? 0) === 0) {
      alerts.push(zh.network.alerts.noInterfaces);
    }
    if (allowSources.length === 0) {
      alerts.push(zh.network.alerts.noSources);
    }
  } else if (statusQuery.data !== undefined && tailnetAddress === null) {
    alerts.push(zh.network.alerts.noRoutes);
  }

  const saveIntranet = async (patch: Partial<HostSettings['intranet']>): Promise<void> => {
    const current = latestSettings.current;
    if (current === undefined) {
      return;
    }

    const optimistic: HostSettings = {
      ...current,
      intranet: { ...current.intranet, ...patch },
    };
    latestSettings.current = optimistic;
    settingsQuery.mutate(optimistic);
    setSaveError(null);

    try {
      const saved = await ipc.call('settings.set', { intranet: patch });
      latestSettings.current = saved;
      settingsQuery.mutate(saved);
    } catch (error) {
      latestSettings.current = current;
      settingsQuery.mutate(current);
      setSaveError(zh.network.alerts.saveFailed(errorMessage(error)));
    }
  };

  const toggleInterface = (name: string, checked: boolean): void => {
    const next = new Set(selectedInterfaces);
    if (checked) {
      next.add(name);
    } else {
      next.delete(name);
    }
    void saveIntranet({ interfaces: nicOrder.filter((item) => next.has(item)) });
  };

  const addSource = (): void => {
    const value = sourceInput.trim();
    if (!isValidCidr(value)) {
      setSourceError(zh.network.intranet.invalidCidr);
      return;
    }
    if (allowSources.includes(value)) {
      setSourceError(zh.network.intranet.duplicateCidr);
      return;
    }

    setSourceError(null);
    setSourceInput('');
    void saveIntranet({ allow_sources: [...allowSources, value] });
  };

  const removeSource = (source: string): void => {
    setSourceError(null);
    void saveIntranet({ allow_sources: allowSources.filter((item) => item !== source) });
  };

  const copyPhoneAddress = async (): Promise<void> => {
    if (phoneAddress === null) {
      return;
    }
    try {
      await phoneCopy.run(() => platform.copyText(phoneAddress ?? ''));
    } catch {
      return;
    }
  };

  const copyItNote = async (): Promise<void> => {
    setItNoteError(null);
    try {
      const note = await ipc.call('network.itNote');
      await itNoteCopy.run(() => platform.copyText(note.text));
    } catch (error) {
      const message = zh.network.intranet.itNoteFailed(errorMessage(error));
      setItNoteError(message);
      window.alert(message);
    }
  };

  return (
    <section className={styles.page} data-section="network">
      <h2 className={styles.title}>{zh.network.title}</h2>
      <p className={styles.intro}>{zh.network.intro}</p>

      {saveError !== null ? (
        <div className={styles.saveError}>
          <Banner role="alert" tone="bad">
            {saveError}
          </Banner>
        </div>
      ) : null}

      {settings === undefined || intranet === undefined ? (
        <p className={styles.loading}>{zh.common.loading}</p>
      ) : (
        <div className={styles.groups}>
          <div className={styles.group}>
            <h3 className={styles.groupTitle}>{zh.network.group.connection}</h3>
            <div className={styles.card}>
              <RowCopy
                title={
                  <span className={styles.titleWithIcon}>
                    <span className={styles.leadingIcon}>
                      <NetworkIcon />
                    </span>
                    {zh.network.tailnet.label}
                  </span>
                }
                description={
                  <span className={styles.tailnetMeta}>
                    <StatusDot tone={tailnetAddress === null ? 'bad' : 'good'} />
                    {tailnetAddress !== null ? (
                      <>
                        <span className={styles.mono}>{tailnetAddress}</span>
                        <span>{` · ${zh.network.tailnet.identity}`}</span>
                      </>
                    ) : (
                      <span>{zh.network.tailnet.identity}</span>
                    )}
                  </span>
                }
              >
                <span className={tailnetAddress === null ? styles.badText : styles.goodText}>
                  {tailnetAddress === null
                    ? zh.network.tailnet.disconnected
                    : zh.network.tailnet.connected}
                </span>
              </RowCopy>

              <RowCopy
                divider
                title={<span id={intranetLabelId}>{zh.network.intranet.label}</span>}
                description={
                  <span className={styles.warningText}>
                    <WarningIcon />
                    {zh.network.intranet.hint}
                  </span>
                }
              >
                <span className={styles.rowStatus}>
                  {enabled ? zh.network.intranet.on : zh.network.intranet.off}
                </span>
                <Switch
                  checked={enabled}
                  labelledBy={intranetLabelId}
                  onChange={(next) => {
                    void saveIntranet({ enabled: next });
                  }}
                />
              </RowCopy>

              {enabled ? (
                <div className={styles.expanded}>
                  <div className={styles.expandedSection}>
                    <div className={styles.fieldTitle}>{zh.network.intranet.interfaces}</div>
                    {nicsQuery.data === undefined ? (
                      <p className={styles.mutedText}>{zh.common.loading}</p>
                    ) : (
                      <div className={styles.nicList}>
                        {nicRows.map((nic) => {
                          const checked = selectedInterfaces.has(nic.name);
                          return (
                            <div className={styles.nicRow} key={nic.name}>
                              <Checkbox
                                checked={checked}
                                description={
                                  nic.missing ? zh.network.intranet.missingNic : undefined
                                }
                                disabled={!nic.missing && nic.address === null}
                                label={<span className={styles.mono}>{nic.name}</span>}
                                onChange={(next) => toggleInterface(nic.name, next)}
                              />
                              <span className={styles.nicAddress}>
                                {nic.missing ? (
                                  <span className={styles.text3}>
                                    {zh.network.intranet.missingNic}
                                  </span>
                                ) : nic.address !== null ? (
                                  <span className={styles.mono}>{nic.address}</span>
                                ) : (
                                  <span className={styles.mutedText}>
                                    {zh.network.intranet.disconnected}
                                  </span>
                                )}
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>

                  <div className={styles.expandedSection}>
                    <div className={styles.splitRow}>
                      <div>
                        <div className={styles.fieldTitle}>{zh.network.intranet.port}</div>
                        <div className={styles.fieldHint}>{zh.network.intranet.portHint}</div>
                      </div>
                      <span className={[styles.mono, styles.portValue].join(' ')}>
                        {zh.network.intranet.portValue}
                      </span>
                    </div>
                  </div>

                  <div className={styles.expandedSection}>
                    <div className={styles.fieldTitle}>{zh.network.intranet.sources}</div>
                    <div className={styles.fieldHint}>{zh.network.intranet.sourcesHint}</div>
                    <div className={styles.sourceList}>
                      {allowSources.map((source) => (
                        <div className={styles.sourceRow} key={source}>
                          <span className={styles.mono}>{source}</span>
                          <Button
                            aria-label={zh.network.intranet.removeSource(source)}
                            disabled={false}
                            onClick={() => removeSource(source)}
                            size="sm"
                            variant="ghost"
                          >
                            {zh.network.intranet.remove}
                          </Button>
                        </div>
                      ))}
                    </div>
                    <div className={styles.sourceForm}>
                      <input
                        aria-label={zh.network.intranet.newSource}
                        className={styles.input}
                        onChange={(event) => {
                          setSourceInput(event.target.value);
                          setSourceError(null);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter') {
                            event.preventDefault();
                            addSource();
                          }
                        }}
                        placeholder={zh.network.intranet.newSourcePlaceholder}
                        type="text"
                        value={sourceInput}
                      />
                      <Button onClick={addSource} size="sm" variant="secondary">
                        {zh.network.intranet.add}
                      </Button>
                    </div>
                    {sourceError !== null ? (
                      <p className={styles.fieldError} role="alert">
                        {sourceError}
                      </p>
                    ) : null}
                  </div>

                  {phoneAddress !== null ? (
                    <div className={styles.expandedSection}>
                      <div className={styles.fieldTitle}>{zh.network.intranet.phoneAddress}</div>
                      <div className={styles.phoneRow}>
                        <span className={[styles.mono, styles.phoneAddress].join(' ')}>
                          {phoneAddress}
                        </span>
                        <Button
                          onClick={() => {
                            void copyPhoneAddress();
                          }}
                          size="sm"
                          variant="secondary"
                        >
                          {phoneCopy.copied ? zh.network.intranet.copied : zh.network.intranet.copy}
                        </Button>
                      </div>
                      <div className={styles.fieldHint}>{zh.network.intranet.phoneAddressHint}</div>
                    </div>
                  ) : null}

                  <div className={styles.expandedSection}>
                    <div className={styles.fieldTitle}>{zh.network.intranet.firewall}</div>
                    {firewallQuery.data === undefined ? null : firewallQuery.data.enabled &&
                      firewallQuery.data.active ? (
                      <div className={styles.firewallGood}>
                        <span className={styles.goodIcon}>
                          <GoodIcon />
                        </span>
                        <span>{zh.network.intranet.firewallActive}</span>
                      </div>
                    ) : firewallQuery.data.enabled ? (
                      <Banner tone="bad" title={zh.network.intranet.firewallBlocked}>
                        {zh.network.intranet.firewallBlockedHint}
                      </Banner>
                    ) : (
                      <p className={styles.mutedText}>{zh.network.intranet.firewallOff}</p>
                    )}
                    <div className={styles.noteActions}>
                      <Button
                        onClick={() => {
                          void copyItNote();
                        }}
                        size="sm"
                        variant="secondary"
                      >
                        {itNoteCopy.copied
                          ? zh.network.intranet.copied
                          : zh.network.intranet.copyItNote}
                      </Button>
                      {itNoteError !== null ? (
                        <span className={styles.fieldError}>{itNoteError}</span>
                      ) : null}
                    </div>
                  </div>
                </div>
              ) : null}
            </div>
          </div>

          <div className={styles.group}>
            <h3 className={styles.groupTitle}>{zh.network.group.pairing}</h3>
            <div className={styles.card}>
              <RowCopy
                title={<span id={confirmEachId}>{zh.network.pairing.confirmEach}</span>}
                description={zh.network.pairing.confirmEachHint}
              >
                <Switch
                  checked={intranet.confirm_each}
                  disabled={!enabled}
                  labelledBy={confirmEachId}
                  onChange={(next) => {
                    void saveIntranet({ confirm_each: next });
                  }}
                />
              </RowCopy>
              <RowCopy
                divider
                title={<span id={allowPairingId}>{zh.network.pairing.allowPairing}</span>}
                description={zh.network.pairing.allowPairingHint}
              >
                <Switch
                  checked={intranet.allow_pairing}
                  disabled={!enabled}
                  labelledBy={allowPairingId}
                  onChange={(next) => {
                    void saveIntranet({ allow_pairing: next });
                  }}
                />
              </RowCopy>
            </div>
          </div>

          {alerts.length > 0 ? (
            <div className={styles.validationList}>
              {alerts.map((alert) => (
                <p className={styles.validation} key={alert} role="alert">
                  {alert}
                </p>
              ))}
            </div>
          ) : null}
        </div>
      )}
    </section>
  );
}
