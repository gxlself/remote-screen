import { useMemo } from 'react';
import AppText from '../../../components/AppText';
import { MIN_HOST_VERSION } from '../../../features/diagnostics/hostVersion';
import { versionInfo } from '../../../features/errors/versionInfo';
import {
  appUpdateUrls,
  openFirst,
} from '../../../features/platform/externalApps';
import type { Computer } from '../../../native/rscore/types';
import { usePlatform } from '../../../native/platform/PlatformContext';
import { common } from '../../../i18n/zh/common';
import { errBoards3 } from '../../../i18n/zh/errBoards3';
import { sessionEnded } from '../../../i18n/zh/sessionEnded';
import type { BoardProps, EndOs } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, StatusRow, summaryText } from '../parts';

const copy = errBoards3.version;

function osVersionOf(host: Computer | undefined, os: EndOs): string | null {
  if (host?.osVersion !== undefined) {
    return host.osVersion;
  }
  return os === 'windows'
    ? copy.card.osFallback.windows
    : os === 'macos'
    ? copy.card.osFallback.mac
    : null;
}

function requireLabel(version: string): string {
  return `≥ ${version}`;
}

function CardSubtitle({
  host,
  hostName,
  os,
}: Pick<BoardProps, 'host' | 'hostName' | 'os'>) {
  return (
    <AppText color="text3" variant="caption">
      {copy.card.subtitle(hostName, osVersionOf(host, os))}
    </AppText>
  );
}

function Version170Board({
  host,
  hostName,
  os,
  code,
  actions,
  params,
}: BoardProps) {
  const platform = usePlatform();
  const appVersion = platform.deviceInfo().appVersion;
  const info = useMemo(
    () =>
      versionInfo({
        code: 170,
        ...(host === undefined ? {} : { host }),
        ...(params.detail === undefined ? {} : { detail: params.detail }),
        appVersion,
        minHost: MIN_HOST_VERSION,
      }),
    [appVersion, host, params.detail],
  );

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={`${copy.desc.old(info.hostVersion ?? '—', info.minHost)}${
        copy.desc.update
      }`}
      illustrationSummary={summaryText([
        { label: copy.row.host, value: info.hostVersion ?? '—' },
        {
          label: copy.row.appRequires(appVersion),
          value: requireLabel(info.minHost),
        },
      ])}
      illustration={
        <MockWindow title={copy.card.mismatch(hostName)}>
          <CardSubtitle host={host} hostName={hostName} os={os} />
          <StatusRow
            label={copy.row.host}
            tone="bad"
            value={info.hostVersion ?? '—'}
          />
          <StatusRow
            label={copy.row.appRequires(appVersion)}
            value={requireLabel(info.minHost)}
          />
        </MockWindow>
      }
      kicker={copy.kicker.old}
      meta={copy.meta.idle(appVersion)}
      onCopyCode={actions.copyCode}
      primary={{ label: copy.action.retry, onPress: () => actions.retry() }}
      secondary={{
        label: sessionEnded.useDesktop(os),
        onPress: () => {
          actions.openScreenSharing().catch(() => undefined);
        },
      }}
      title={copy.title.old(hostName)}
    />
  );
}

function Version171Board({
  host,
  hostName,
  os,
  code,
  actions,
  params,
}: BoardProps) {
  const platform = usePlatform();
  const appVersion = platform.deviceInfo().appVersion;
  const info = useMemo(
    () =>
      versionInfo({
        code: 171,
        ...(host === undefined ? {} : { host }),
        ...(params.detail === undefined ? {} : { detail: params.detail }),
        appVersion,
        minHost: MIN_HOST_VERSION,
      }),
    [appVersion, host, params.detail],
  );

  const update = () => {
    openFirst(platform, appUpdateUrls()).then(opened => {
      if (opened === null) {
        actions.toast(copy.updateUnavailable);
      }
    });
  };

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={copy.desc.appOld}
      illustrationSummary={summaryText([
        { label: copy.row.app, value: appVersion },
        ...(info.minApp === null
          ? []
          : [
              {
                label: copy.row.hostRequires(hostName),
                value: requireLabel(info.minApp),
              },
            ]),
      ])}
      illustration={
        <MockWindow title={copy.card.mismatch(hostName)}>
          <CardSubtitle host={host} hostName={hostName} os={host?.os ?? os} />
          <StatusRow label={copy.row.app} tone="bad" value={appVersion} />
          {info.minApp === null ? null : (
            <StatusRow
              label={copy.row.hostRequires(hostName)}
              value={requireLabel(info.minApp)}
            />
          )}
        </MockWindow>
      }
      kicker={copy.kicker.appOld}
      meta={copy.meta.idle(appVersion)}
      onCopyCode={actions.copyCode}
      primary={{ label: copy.action.updateApp, onPress: update }}
      title={copy.title.appOld(hostName)}
    />
  );
}

export default function VersionBoard(props: BoardProps) {
  return props.code === 171 ? (
    <Version171Board {...props} />
  ) : (
    <Version170Board {...props} />
  );
}
