import { useEffect, useMemo, useState, type ReactNode } from 'react';

import type { PageProps } from '../../app/router';
import { SETUP_DONE_KEY, writeFlag } from '../../app/storage';
import { AppGlyph, Banner, Button, Checkbox } from '../../components';
import { zhFor } from '../../i18n/winText';
import { useIpc, useIpcConnected, useIpcQuery } from '../../ipc';
import { firstIPv4, isValidCidr } from '../../lib';
import { SYSTEM_SETTINGS_URLS, usePlatform } from '../../platform';
import type { HostOs } from '../../platform/types';

import styles from './setup.module.css';

const MAC_STEP_IDS = ['tailnet', 'screen', 'ax', 'pair'] as const;
const WINDOWS_STEP_IDS = ['tailnet', 'service', 'lan', 'pair'] as const;

type MacStepId = (typeof MAC_STEP_IDS)[number];
type WindowsStepId = (typeof WINDOWS_STEP_IDS)[number];
type StepId = MacStepId | WindowsStepId;
type SetupCopy = ReturnType<typeof zhFor>;

interface NicView {
  name: string;
  address: string | null;
  tailnet: boolean;
}

function stepsFor(os: HostOs): readonly StepId[] {
  return os === 'windows' ? WINDOWS_STEP_IDS : MAC_STEP_IDS;
}

function firstIncomplete(completed: readonly boolean[]): number {
  const index = completed.findIndex((done) => !done);
  return index === -1 ? completed.length : index;
}

function nextIncomplete(completed: readonly boolean[], currentStep: number): number {
  for (let index = currentStep + 1; index < completed.length; index += 1) {
    if (completed[index] !== true) {
      return index;
    }
  }
  for (let index = 0; index < currentStep; index += 1) {
    if (completed[index] !== true) {
      return index;
    }
  }
  return completed.length;
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

function firstSelectedInterface(
  nics: readonly NicView[],
  selected: readonly string[],
): string | null {
  const selectedNames = new Set(selected);
  const visible = nics.find((nic) => !nic.tailnet && selectedNames.has(nic.name));
  return visible?.name ?? selected[0] ?? null;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function CheckIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="12" viewBox="0 0 12 12" width="12">
      <path
        d="M2.25 6.2 4.7 8.6 9.75 3.4"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.8"
      />
    </svg>
  );
}

function Spinner() {
  return <span aria-hidden="true" className={styles.spinner} />;
}

function MacIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="24" viewBox="0 0 24 24" width="24">
      <rect height="13" rx="2" stroke="currentColor" strokeWidth="1.5" width="19" x="2.5" y="3.5" />
      <path
        d="M8.5 20h7M12 16.5V20"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.5"
      />
    </svg>
  );
}

function DesktopIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="26" viewBox="0 0 26 26" width="26">
      <rect height="15" rx="2" stroke="currentColor" strokeWidth="1.5" width="21" x="2.5" y="3.5" />
      <path
        d="M8.5 22h9M13 18.5V22"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.5"
      />
      <path d="M6 15.5h14" stroke="currentColor" strokeWidth="1.2" />
    </svg>
  );
}

function PhoneIcon() {
  return (
    <svg aria-hidden="true" fill="none" height="34" viewBox="0 0 24 24" width="34">
      <rect height="19" rx="3" stroke="currentColor" strokeWidth="1.5" width="11" x="6.5" y="2.5" />
      <path d="M10 18.5h4" stroke="currentColor" strokeLinecap="round" strokeWidth="1.5" />
    </svg>
  );
}

function PermissionVisual({ settingsPage }: { settingsPage: string }) {
  const copy = zhFor('macos');
  return (
    <div aria-hidden="true" className={styles.visual}>
      <div className={styles.settingsPath}>
        <span>{copy.setup.visual.systemSettings}</span>
        <span className={styles.chevron}>›</span>
        <span>{copy.setup.visual.privacy}</span>
        <span className={styles.chevron}>›</span>
        <strong>{settingsPage}</strong>
      </div>
      <div className={styles.permissionRow}>
        <span className={styles.permissionIcon}>
          <MacIcon />
        </span>
        <span className={styles.permissionApp}>{copy.setup.visual.appName}</span>
        <span className={styles.switchTrack}>
          <span className={styles.switchKnob} />
        </span>
      </div>
      <span className={styles.callout}>{copy.setup.visual.switchHint}</span>
    </div>
  );
}

function TailnetVisual({ label }: { label: string }) {
  return (
    <div aria-hidden="true" className={`${styles.visual} ${styles.tailnetVisual}`}>
      <span className={styles.deviceTile}>
        <MacIcon />
      </span>
      <span className={styles.connectionLine}>
        <span className={styles.connectionDot} />
      </span>
      <span className={styles.tailnetBadge}>{label}</span>
    </div>
  );
}

function ServiceVisual({ running }: { running: boolean }) {
  return (
    <div aria-hidden="true" className={`${styles.visual} ${styles.serviceVisual}`}>
      <span className={styles.serviceTile}>
        <DesktopIcon />
      </span>
      <span className={styles.connectionLine}>
        <span className={running ? styles.connectionDot : styles.connectionDotStopped} />
      </span>
      <span className={`${styles.serviceBadge} ${running ? styles.serviceBadgeGood : ''}`}>
        <span className={styles.serviceBadgeDot} />
      </span>
    </div>
  );
}

function PairVisual() {
  return (
    <div aria-hidden="true" className={`${styles.visual} ${styles.pairVisual}`}>
      <span className={styles.qrTile}>
        {Array.from({ length: 9 }, (_, index) => (
          <span className={index % 3 === 1 ? styles.qrDark : styles.qrLight} key={index} />
        ))}
      </span>
      <span className={styles.pairPlus}>+</span>
      <span className={styles.phoneTile}>
        <PhoneIcon />
      </span>
    </div>
  );
}

function StepLayout({
  progress,
  title,
  description,
  children,
  footer,
}: {
  progress: string;
  title: string;
  description: string;
  children: ReactNode;
  footer: ReactNode;
}) {
  return (
    <>
      <div className={styles.progress}>{progress}</div>
      <h2 className={styles.title}>{title}</h2>
      <p className={styles.description}>{description}</p>
      {children}
      <div className={styles.spacer} />
      {footer}
    </>
  );
}

function StepFooter({ hint, children }: { hint?: string; children: ReactNode }) {
  return (
    <footer className={styles.footer}>
      <span className={styles.footerHint}>{hint}</span>
      <div className={styles.footerActions}>{children}</div>
    </footer>
  );
}

function macStatusCopy(
  copy: SetupCopy,
  step: MacStepId,
  completed: boolean,
  current: boolean,
  tailnetAddress: string,
  pairedCount: number,
): string {
  const steps = copy.setup.steps;
  switch (step) {
    case 'tailnet':
      return completed ? steps.tailnet.done(tailnetAddress) : steps.tailnet.pending;
    case 'screen':
      if (completed) {
        return steps.screen.done;
      }
      return current ? steps.screen.waiting : steps.screen.pending;
    case 'ax':
      if (completed) {
        return steps.ax.done;
      }
      return current ? steps.ax.waiting : steps.ax.pending;
    case 'pair':
      return completed ? steps.pair.done(pairedCount) : steps.pair.pending;
  }
}

function windowsStatusCopy(
  copy: SetupCopy,
  step: WindowsStepId,
  completed: boolean,
  lanFormOpen: boolean,
  lanSkipped: boolean,
  lanNicName: string | null,
  tailnetAddress: string,
  pairedCount: number,
): string {
  const steps = copy.setup.windows.steps;
  switch (step) {
    case 'tailnet':
      return completed ? steps.tailnet.done(null, tailnetAddress) : steps.tailnet.pending;
    case 'service':
      return completed ? steps.service.done : steps.service.pending;
    case 'lan':
      if (completed) {
        return lanSkipped ? steps.lan.skipped : steps.lan.done(lanNicName ?? '');
      }
      return lanFormOpen ? steps.lan.unsaved : steps.lan.pending;
    case 'pair':
      return completed ? steps.pair.done(pairedCount) : steps.pair.pending;
  }
}

function StepMarker({ done, current, index }: { done: boolean; current: boolean; index: number }) {
  if (done) {
    return (
      <span aria-hidden="true" className={`${styles.marker} ${styles.markerDone}`}>
        <CheckIcon />
      </span>
    );
  }

  return (
    <span
      aria-hidden="true"
      className={`${styles.marker} ${current ? styles.markerCurrent : styles.markerPending}`}
    >
      {index + 1}
    </span>
  );
}

function WaitingStatus({ children }: { children: ReactNode }) {
  return (
    <span className={styles.status} role="status">
      <Spinner />
      {children}
    </span>
  );
}

export default function SetupPage(_props: PageProps) {
  const platform = usePlatform();
  const copy = zhFor(platform.os);
  const isWindows = platform.os === 'windows';
  const steps = stepsFor(platform.os);
  const ipc = useIpc();
  const connected = useIpcConnected();
  const [selection, setSelection] = useState<{
    step: number;
    completedAtSelection: boolean;
  } | null>(null);
  const [lanFormOpen, setLanFormOpen] = useState(false);
  const [lanSkipped, setLanSkipped] = useState(false);
  const [selectedInterfaces, setSelectedInterfaces] = useState<string[]>([]);
  const [allowSources, setAllowSources] = useState<string[]>([]);
  const [sourceInput, setSourceInput] = useState('');
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const statusQuery = useIpcQuery('status.get', {
    refetchOn: ['event.permissionsChanged'],
  });
  const devicesQuery = useIpcQuery('devices.list', { refetchOn: ['event.pairingResult'] });
  const settingsQuery = useIpcQuery('settings.get', { enabled: isWindows });
  const nicsQuery = useIpcQuery('network.listNics', { enabled: isWindows });
  const refetchStatus = statusQuery.refetch;

  const tailnetAddresses = statusQuery.data?.tailnet_addresses ?? [];
  const tailnetAddress = tailnetAddresses[0] ?? '';
  const pairedCount = devicesQuery.data?.devices.length ?? 0;
  const tailnetDone = tailnetAddresses.length > 0;
  const screenDone = statusQuery.data?.permissions?.screen_recording === true;
  const axDone = statusQuery.data?.permissions?.accessibility === true;
  const pairDone = pairedCount > 0;
  const serviceDone = statusQuery.data !== undefined && statusQuery.error === undefined;

  const configuredIntranet = settingsQuery.data?.intranet;
  const configuredEnabled = configuredIntranet?.enabled === true;
  const lanDone = configuredEnabled || lanSkipped;
  const intranetOpen = configuredEnabled;
  const savedNicInterfaces =
    !lanFormOpen && configuredEnabled ? (configuredIntranet?.interfaces ?? []) : [];

  const nics = parseNics(nicsQuery.data?.nics);
  const visibleNics = nics.filter((nic) => !nic.tailnet);
  const firstNic = visibleNics.find((nic) => nic.address !== null) ?? null;
  const savedNicName = firstSelectedInterface(nics, savedNicInterfaces);
  const nicsLoading = nicsQuery.data === undefined && nicsQuery.error === undefined;

  const completed = useMemo<readonly boolean[]>(
    () =>
      isWindows
        ? [tailnetDone, serviceDone, lanDone, pairDone]
        : [tailnetDone, screenDone, axDone, pairDone],
    [axDone, isWindows, lanDone, pairDone, screenDone, serviceDone, tailnetDone],
  );

  const ready = isWindows
    ? !statusQuery.loading && !devicesQuery.loading && !settingsQuery.loading
    : statusQuery.data !== undefined && devicesQuery.data !== undefined;
  const initialStep = ready ? firstIncomplete(completed) : null;
  const currentStep =
    selection === null
      ? initialStep
      : !selection.completedAtSelection && completed[selection.step] === true
        ? nextIncomplete(completed, selection.step)
        : selection.step;

  const currentStepId: StepId | undefined =
    currentStep !== null && currentStep < steps.length ? steps[currentStep] : undefined;
  const currentDone =
    currentStep !== null && currentStep < steps.length ? completed[currentStep] === true : false;
  const allDone = completed.every((done) => done);

  useEffect(() => {
    if (currentStep === null || currentStep >= steps.length - 1) {
      return;
    }
    const timer = setInterval(() => {
      void refetchStatus();
    }, 2_000);
    return () => clearInterval(timer);
  }, [currentStep, refetchStatus, steps.length]);

  const selectStep = (step: number): void => {
    setSelection({ step, completedAtSelection: completed[step] === true });
  };

  const goBack = (): void => {
    if (currentStep !== null && currentStep > 0 && currentStep <= steps.length) {
      const previousStep = currentStep - 1;
      setSelection({
        step: previousStep,
        completedAtSelection: completed[previousStep] === true,
      });
    }
  };

  const goNext = (): void => {
    if (currentStep === null) {
      return;
    }
    if (currentStep < steps.length - 1) {
      const nextStep = currentStep + 1;
      setSelection({
        step: nextStep,
        completedAtSelection: completed[nextStep] === true,
      });
      return;
    }
    setSelection({ step: steps.length, completedAtSelection: allDone });
  };

  const openSettings = (url: string): void => {
    void platform.openUrl(url);
  };

  const showPairing = (): void => {
    void platform.showWindow('pair', 'pair');
  };

  const hideForNow = (): void => {
    void platform.hideWindow('setup');
  };

  const finish = (): void => {
    writeFlag(SETUP_DONE_KEY, '1');
    void platform.hideWindow('setup');
  };

  const openIntranet = (): void => {
    const currentIntranet = settingsQuery.data?.intranet;
    const initialInterfaces = configuredEnabled ? (currentIntranet?.interfaces ?? []) : [];
    const initialSources = configuredEnabled ? (currentIntranet?.allow_sources ?? []) : [];
    setLanFormOpen(true);
    setSelectedInterfaces(initialInterfaces);
    setAllowSources(initialSources);
    setSourceInput('');
    setSourceError(null);
    setSaveError(null);
  };

  const leaveIntranetForm = (): void => {
    setLanFormOpen(false);
    setSelectedInterfaces([]);
    setAllowSources([]);
    setSourceInput('');
    setSourceError(null);
  };

  const skipIntranet = (): void => {
    setLanSkipped(true);
    leaveIntranetForm();
    const lanIndex = steps.indexOf('lan');
    const nextStep = lanIndex + 1;
    setSelection({
      step: Math.min(nextStep, steps.length),
      completedAtSelection: nextStep < steps.length && completed[nextStep] === true,
    });
  };

  const toggleInterface = (name: string, checked: boolean): void => {
    setSelectedInterfaces((current) =>
      checked ? [...current, name] : current.filter((item) => item !== name),
    );
  };

  const addSource = (): void => {
    const value = sourceInput.trim();
    if (!isValidCidr(value)) {
      setSourceError(copy.network.intranet.invalidCidr);
      return;
    }
    if (allowSources.includes(value)) {
      setSourceError(copy.network.intranet.duplicateCidr);
      return;
    }

    setSourceError(null);
    setSourceInput('');
    setAllowSources((current) => [...current, value]);
  };

  const removeSource = (source: string): void => {
    setSourceError(null);
    setAllowSources((current) => current.filter((item) => item !== source));
  };

  const saveIntranet = async (): Promise<void> => {
    if (selectedInterfaces.length === 0 || allowSources.length === 0 || saving) {
      return;
    }

    setSaving(true);
    setSaveError(null);
    const interfaces = [...selectedInterfaces];
    const sources = [...allowSources];
    try {
      const saved = await ipc.call('settings.set', {
        intranet: {
          enabled: true,
          interfaces,
          allow_sources: sources,
        },
      });
      settingsQuery.mutate(saved);
      setSourceInput('');
      setLanFormOpen(false);
      const lanIndex = steps.indexOf('lan');
      const nextStep = lanIndex + 1;
      setSelection({
        step: Math.min(nextStep, steps.length),
        completedAtSelection: nextStep < steps.length && completed[nextStep] === true,
      });
    } catch (error) {
      setSaveError(copy.network.alerts.saveFailed(errorMessage(error)));
    } finally {
      setSaving(false);
    }
  };

  const laterPairing = (): void => {
    setSelection({ step: steps.length, completedAtSelection: true });
  };

  let content: ReactNode;
  if (currentStep === null) {
    content = (
      <div className={styles.loading} role="status">
        {copy.common.loading}
      </div>
    );
  } else if (currentStep === steps.length) {
    content = (
      <div className={styles.complete}>
        <span aria-hidden="true" className={styles.completeCheck}>
          <CheckIcon />
        </span>
        <h2 className={styles.completeTitle}>{copy.setup.complete.title}</h2>
        <p className={styles.completeDescription}>
          {isWindows ? copy.setup.windows.complete.description : copy.setup.complete.description}
        </p>
        <Button onClick={finish}>{copy.common.done}</Button>
      </div>
    );
  } else if (isWindows && currentStepId !== undefined) {
    const windowsCopy = copy.setup.windows;
    const progress = copy.setup.progress(currentStep + 1, steps.length);

    if (currentStepId === 'tailnet') {
      const stepCopy = windowsCopy.content.tailnet;
      content = (
        <StepLayout
          description={stepCopy.description}
          footer={
            <StepFooter>
              <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                {copy.common.back}
              </Button>
              {currentDone ? <Button onClick={goNext}>{copy.common.next}</Button> : null}
            </StepFooter>
          }
          progress={progress}
          title={stepCopy.title}
        >
          <TailnetVisual label={copy.setup.visual.tailnet} />
          {currentDone ? (
            <Banner tone="good">{copy.setup.stepDone}</Banner>
          ) : (
            <div className={styles.actionRow}>
              <WaitingStatus>{stepCopy.waiting}</WaitingStatus>
            </div>
          )}
        </StepLayout>
      );
    } else if (currentStepId === 'service') {
      const stepCopy = windowsCopy.content.service;
      content = (
        <StepLayout
          description={stepCopy.description}
          footer={
            <StepFooter>
              <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                {copy.common.back}
              </Button>
              {currentDone ? <Button onClick={goNext}>{copy.common.next}</Button> : null}
            </StepFooter>
          }
          progress={progress}
          title={stepCopy.title}
        >
          <ServiceVisual running={serviceDone} />
          {currentDone ? (
            <Banner tone="good">{copy.setup.stepDone}</Banner>
          ) : (
            <Banner role="alert" title={stepCopy.stopped} tone="bad">
              {stepCopy.hint}
            </Banner>
          )}
        </StepLayout>
      );
    } else if (currentStepId === 'lan') {
      const stepCopy = windowsCopy.content.lan;
      if (currentDone && !lanFormOpen) {
        content = (
          <StepLayout
            description={
              lanSkipped
                ? windowsCopy.steps.lan.skipped
                : windowsCopy.steps.lan.done(savedNicName ?? '')
            }
            footer={
              <StepFooter>
                <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                  {copy.common.back}
                </Button>
                {!lanSkipped ? (
                  <Button onClick={openIntranet} variant="secondary">
                    {stepCopy.edit}
                  </Button>
                ) : null}
                <Button onClick={goNext}>{copy.common.next}</Button>
              </StepFooter>
            }
            progress={progress}
            title={windowsCopy.steps.lan.title}
          >
            <Banner tone="good">{copy.setup.stepDone}</Banner>
          </StepLayout>
        );
      } else if (!lanFormOpen) {
        content = (
          <StepLayout
            description={stepCopy.introDescription}
            footer={
              <StepFooter hint={stepCopy.settingsHint}>
                <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                  {copy.common.back}
                </Button>
                <Button onClick={skipIntranet} variant="ghost">
                  {stepCopy.skip}
                </Button>
                <Button onClick={openIntranet}>{stepCopy.open}</Button>
              </StepFooter>
            }
            progress={progress}
            title={stepCopy.introTitle}
          >
            <section className={styles.infoSection}>
              <h3 className={styles.infoTitle}>{stepCopy.whenTitle}</h3>
              <div className={styles.reasonGrid}>
                {stepCopy.whenCards.map((card) => (
                  <div className={styles.reasonCard} key={card.title}>
                    <strong>{card.title}</strong>
                    <span>{card.description}</span>
                  </div>
                ))}
              </div>
            </section>

            <section className={styles.infoSection}>
              <h3 className={styles.infoTitle}>{stepCopy.choicesTitle}</h3>
              <ul className={styles.choiceList}>
                <li>
                  <strong>{stepCopy.interfacesLabel}</strong>
                  <span>
                    {firstNic !== null
                      ? `${firstNic.name} · ${firstNic.address}`
                      : nicsLoading
                        ? stepCopy.loadingNics
                        : stepCopy.noNics}
                  </span>
                </li>
                <li>
                  <strong>{stepCopy.sourcesLabel}</strong>
                  <span>{stepCopy.sourcesExample}</span>
                </li>
                <li>
                  <strong>{stepCopy.firewallLabel}</strong>
                  <span>{stepCopy.firewallValue}</span>
                </li>
              </ul>
            </section>

            <p className={styles.warningText}>{stepCopy.warning}</p>
          </StepLayout>
        );
      } else {
        const saveDisabled = selectedInterfaces.length === 0 || allowSources.length === 0 || saving;
        content = (
          <StepLayout
            description={stepCopy.openDescription}
            footer={
              <StepFooter hint={stepCopy.settingsHint}>
                <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                  {copy.common.back}
                </Button>
                <Button onClick={skipIntranet} variant="ghost">
                  {stepCopy.skip}
                </Button>
                <Button disabled={saveDisabled} onClick={() => void saveIntranet()}>
                  {stepCopy.save}
                </Button>
              </StepFooter>
            }
            progress={progress}
            title={stepCopy.openTitle}
          >
            {saveError !== null ? (
              <Banner role="alert" tone="bad">
                {saveError}
              </Banner>
            ) : null}

            <section className={styles.formSection}>
              <h3 className={styles.formTitle}>{stepCopy.formInterfaces}</h3>
              {nicsLoading ? (
                <p className={styles.mutedText}>{stepCopy.loadingNics}</p>
              ) : visibleNics.length === 0 ? (
                <p className={styles.mutedText}>{stepCopy.noNics}</p>
              ) : (
                <div className={styles.nicList}>
                  {visibleNics.map((nic) => (
                    <div className={styles.nicRow} key={nic.name}>
                      <Checkbox
                        checked={selectedInterfaces.includes(nic.name)}
                        disabled={nic.address === null}
                        label={nic.name}
                        onChange={(checked) => toggleInterface(nic.name, checked)}
                      />
                      <span className={nic.address === null ? styles.mutedText : styles.mono}>
                        {nic.address ?? copy.network.intranet.disconnected}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </section>

            <section className={styles.formSection}>
              <h3 className={styles.formTitle}>{stepCopy.formSources}</h3>
              <div className={styles.sourceList}>
                {allowSources.map((source) => (
                  <div className={styles.sourceRow} key={source}>
                    <span className={styles.mono}>{source}</span>
                    <Button
                      aria-label={copy.network.intranet.removeSource(source)}
                      onClick={() => removeSource(source)}
                      size="sm"
                      variant="ghost"
                    >
                      {copy.network.intranet.remove}
                    </Button>
                  </div>
                ))}
              </div>
              <div className={styles.sourceForm}>
                <input
                  aria-label={copy.network.intranet.newSource}
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
                  placeholder={copy.network.intranet.newSourcePlaceholder}
                  type="text"
                  value={sourceInput}
                />
                <Button onClick={addSource} size="sm" variant="secondary">
                  {copy.network.intranet.add}
                </Button>
              </div>
              {sourceError !== null ? (
                <p className={styles.fieldError} role="alert">
                  {sourceError}
                </p>
              ) : null}
            </section>

            <div className={styles.staticRow}>
              <span>{stepCopy.formPort}</span>
              <span className={styles.mono}>{stepCopy.formPortValue}</span>
            </div>
            <div className={styles.staticRow}>
              <span>{stepCopy.formFirewall}</span>
              <span>{stepCopy.formFirewallValue}</span>
            </div>
          </StepLayout>
        );
      }
    } else {
      const stepCopy = windowsCopy.content.pair;
      content = (
        <StepLayout
          description={stepCopy.description}
          footer={
            <StepFooter hint={stepCopy.passwordHint}>
              <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
                {copy.common.back}
              </Button>
              {currentDone ? <Button onClick={goNext}>{copy.common.next}</Button> : null}
            </StepFooter>
          }
          progress={progress}
          title={stepCopy.title}
        >
          <PairVisual />
          <p className={styles.pathHint}>
            {intranetOpen ? stepCopy.lanHint : stepCopy.tailnetHint}
          </p>
          {currentDone ? (
            <Banner tone="good">{copy.setup.stepDone}</Banner>
          ) : (
            <div className={styles.actionRow}>
              <Button onClick={showPairing}>{stepCopy.action}</Button>
              <Button onClick={laterPairing} variant="secondary">
                {stepCopy.later}
              </Button>
            </div>
          )}
        </StepLayout>
      );
    }
  } else if (currentStepId !== undefined) {
    const stepCopy = copy.setup.content[currentStepId as MacStepId];
    let visual: ReactNode;
    if (currentStepId === 'tailnet') {
      visual = <TailnetVisual label={copy.setup.visual.tailnet} />;
    } else if (currentStepId === 'screen') {
      visual = <PermissionVisual settingsPage={copy.setup.visual.screenRecording} />;
    } else if (currentStepId === 'ax') {
      visual = <PermissionVisual settingsPage={copy.setup.visual.accessibility} />;
    } else {
      visual = <PairVisual />;
    }

    let action: ReactNode;
    if (currentDone) {
      action = <Banner tone="good">{copy.setup.stepDone}</Banner>;
    } else if (currentStepId === 'tailnet') {
      action = (
        <div className={styles.actionRow}>
          <WaitingStatus>{copy.setup.content.tailnet.waiting}</WaitingStatus>
        </div>
      );
    } else if (currentStepId === 'screen') {
      action = (
        <div className={styles.actionRow}>
          <Button onClick={() => openSettings(SYSTEM_SETTINGS_URLS.screenRecording)}>
            {copy.setup.content.screen.action}
          </Button>
          <WaitingStatus>{copy.setup.content.screen.waiting}</WaitingStatus>
        </div>
      );
    } else if (currentStepId === 'ax') {
      action = (
        <div className={styles.actionRow}>
          <Button onClick={() => openSettings(SYSTEM_SETTINGS_URLS.accessibility)}>
            {copy.setup.content.ax.action}
          </Button>
          <WaitingStatus>{copy.setup.content.ax.waiting}</WaitingStatus>
        </div>
      );
    } else {
      action = (
        <div className={styles.actionRow}>
          <Button onClick={showPairing}>{copy.setup.content.pair.action}</Button>
        </div>
      );
    }

    const footerHint =
      currentStepId === 'screen'
        ? copy.setup.content.screen.hint
        : currentStepId === 'ax'
          ? copy.setup.content.ax.hint
          : undefined;

    content = (
      <StepLayout
        description={stepCopy.description}
        footer={
          <StepFooter hint={footerHint}>
            <Button disabled={currentStep === 0} onClick={goBack} variant="secondary">
              {copy.common.back}
            </Button>
            <Button onClick={hideForNow} variant="ghost">
              {copy.common.later}
            </Button>
            {currentDone ? <Button onClick={goNext}>{copy.common.next}</Button> : null}
          </StepFooter>
        }
        progress={copy.setup.progress(currentStep + 1, steps.length)}
        title={stepCopy.title}
      >
        {visual}
        {action}
      </StepLayout>
    );
  }

  return (
    <main className={styles.page}>
      <aside className={styles.sidebar}>
        <div className={styles.dragRegion} data-tauri-drag-region />
        <div className={styles.sidebarBody}>
          <header className={styles.brand}>
            <AppGlyph size={34} />
            <div className={styles.brandText}>
              <h1 className={styles.brandTitle}>{copy.setup.sidebar.title}</h1>
              <p className={styles.brandSubtitle}>{copy.setup.sidebar.subtitle}</p>
            </div>
          </header>

          <ol aria-label={copy.setup.sidebar.stepsLabel} className={styles.stepList}>
            {steps.map((step, index) => {
              const done = completed[index] === true;
              const current = currentStep === index;
              const subtitle = isWindows
                ? windowsStatusCopy(
                    copy,
                    step as WindowsStepId,
                    done,
                    lanFormOpen,
                    lanSkipped,
                    savedNicName,
                    tailnetAddress,
                    pairedCount,
                  )
                : macStatusCopy(
                    copy,
                    step as MacStepId,
                    done,
                    current,
                    tailnetAddress,
                    pairedCount,
                  );
              const title = isWindows
                ? copy.setup.windows.steps[step as WindowsStepId].title
                : copy.setup.steps[step as MacStepId].title;

              return (
                <li key={step}>
                  <button
                    aria-current={current ? 'step' : undefined}
                    className={`${styles.stepButton} ${current ? styles.stepButtonCurrent : ''}`}
                    onClick={() => selectStep(index)}
                    type="button"
                  >
                    <StepMarker current={current} done={done} index={index} />
                    <span className={styles.stepText}>
                      <span className={styles.stepTitle}>{title}</span>
                      <span
                        className={`${styles.stepSubtitle} ${current ? styles.stepSubtitleCurrent : ''}`}
                      >
                        {subtitle}
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ol>

          <p className={styles.sidebarNote}>
            {isWindows ? copy.setup.windows.sidebar.note : copy.setup.sidebar.note}
          </p>
        </div>
      </aside>

      <section className={styles.content}>
        {!connected ? <Banner tone="bad">{copy.common.agentDown}</Banner> : null}
        {content}
      </section>
    </main>
  );
}
