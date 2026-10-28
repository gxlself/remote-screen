import { useState } from 'react';
import { StyleSheet, View } from 'react-native';
import Dialog from '../../../components/Dialog';
import { errorCodeLabel } from '../../../features/format/misc';
import { formatMs } from '../../../features/format/latency';
import { formatRelativePast } from '../../../features/format/time';
import { useTailnetEnabled } from '../../../features/tailnet/enabled';
import { common } from '../../../i18n/zh/common';
import { errBoards1 } from '../../../i18n/zh/errBoards1';
import { sessionEnded } from '../../../i18n/zh/sessionEnded';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, NoteBox, StatusRow, summaryText } from '../parts';

type Outcome = 'timeout' | 'refused' | 'ok';

function resultOf(value: string | undefined): Outcome | undefined {
  return value === 'timeout' || value === 'refused' || value === 'ok'
    ? value
    : undefined;
}

function resultValue(copy: typeof errBoards1.port, value: Outcome): string {
  switch (value) {
    case 'timeout':
      return copy.timeout;
    case 'refused':
      return copy.refused;
    case 'ok':
      return copy.reachable;
  }
}

export default function PortBoard({
  params,
  code,
  host,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards1.port;
  const tailnetEnabled = useTailnetEnabled();
  const reported = tailnetEnabled ? copy.reported : copy.reportedLan;
  const mac = os === 'macos';
  const [dialogVisible, setDialogVisible] = useState(false);
  const detail = params.detail ?? {};
  const vpnAddr = detail.vpn_addr;
  const route = detail.route;
  const pingMs =
    detail.ping_ms !== undefined && Number.isFinite(Number(detail.ping_ms))
      ? Number(detail.ping_ms)
      : undefined;
  const udp = resultOf(detail.udp);
  const tcp = resultOf(detail.tcp);
  const hostIntranet =
    detail.host_intranet === 'on' || detail.host_intranet === 'off'
      ? detail.host_intranet
      : undefined;
  const reportedAtUs =
    detail.reported_at_us !== undefined &&
    Number.isFinite(Number(detail.reported_at_us))
      ? Number(detail.reported_at_us)
      : undefined;
  const hasAny =
    vpnAddr !== undefined ||
    route !== undefined ||
    pingMs !== undefined ||
    udp !== undefined ||
    tcp !== undefined ||
    hostIntranet !== undefined ||
    reportedAtUs !== undefined;
  const shownUdp = hasAny ? udp : 'timeout';
  const shownTcp = hasAny ? tcp : 'timeout';
  const address =
    host?.addresses.find(item => item.kind === 'intranet')?.addr ??
    host?.addresses.find(item => item.kind === 'tailnet')?.addr ??
    vpnAddr ??
    hostName;

  const rows: { label: string; value: string; tone: 'good' | 'bad' }[] = [];
  if (vpnAddr !== undefined) {
    rows.push({ label: copy.vpnLabel, value: vpnAddr, tone: 'good' });
  }
  if (route !== undefined) {
    rows.push({
      label: copy.routeLabel,
      value: copy.routeValue(route),
      tone: 'good',
    });
  }
  if (pingMs !== undefined) {
    rows.push({
      label: copy.pingLabel,
      value: formatMs(pingMs),
      tone: 'good',
    });
  }
  if (shownUdp !== undefined) {
    rows.push({
      label: copy.udpLabel,
      value: resultValue(copy, shownUdp),
      tone: shownUdp === 'ok' ? 'good' : 'bad',
    });
  }
  if (shownTcp !== undefined) {
    rows.push({
      label: copy.tcpLabel,
      value: resultValue(copy, shownTcp),
      tone: shownTcp === 'ok' ? 'good' : 'bad',
    });
  }
  if (hostIntranet !== undefined) {
    rows.push({
      label: copy.hostIntranetLabel,
      value: hostIntranet === 'on' ? copy.opened : copy.closed,
      tone: hostIntranet === 'on' ? 'good' : 'bad',
    });
  }

  const desc =
    tcp === 'refused' ? `${copy.desc}${copy.refusedDesc(mac)}` : copy.desc;
  const diagnosticText = [
    `${copy.diagnostics.hostName}：${hostName}`,
    `${copy.diagnostics.address}：${address}`,
    ...rows.map(row => `${row.label}：${row.value}`),
    code === null
      ? copy.diagnostics.errorCode
      : `${copy.diagnostics.errorCode}：${errorCodeLabel(code)}`,
  ].join('\n');
  const illustrationSummary = summaryText(
    rows,
    reportedAtUs === undefined
      ? undefined
      : [reported(formatRelativePast(reportedAtUs, Date.now() * 1000))],
  );

  return (
    <>
      <ErrorFrame
        back={{ label: common.back, onPress: actions.back }}
        code={code}
        desc={desc}
        illustrationSummary={illustrationSummary}
        illustration={
          <MockWindow dots={false} title={copy.windowTitle(address)}>
            <View style={styles.rows}>
              {rows.map(row => (
                <StatusRow
                  key={row.label}
                  label={row.label}
                  tone={row.tone}
                  value={row.value}
                />
              ))}
            </View>
            {reportedAtUs === undefined ? null : (
              <NoteBox
                text={reported(
                  formatRelativePast(reportedAtUs, Date.now() * 1000),
                )}
              />
            )}
          </MockWindow>
        }
        kicker={copy.kicker}
        more={[
          {
            label: copy.copyDiagnostics,
            onPress: () => actions.copyText(diagnosticText, copy.copiedToast),
          },
          {
            label: sessionEnded.useDesktop(os),
            onPress: () => {
              Promise.resolve(actions.openScreenSharing()).catch(
                () => undefined,
              );
            },
          },
        ]}
        onCopyCode={actions.copyCode}
        primary={{ label: common.retry, onPress: () => actions.retry() }}
        secondary={{
          label: copy.howTo(mac),
          onPress: () => setDialogVisible(true),
        }}
        title={copy.title(hostName)}
      />
      <Dialog
        actions={[
          {
            label: copy.gotIt,
            onPress: () => setDialogVisible(false),
          },
        ]}
        message={copy.dialogBody(mac)}
        onDismiss={() => setDialogVisible(false)}
        title={copy.dialogTitle(mac)}
        visible={dialogVisible}
      />
    </>
  );
}

const styles = StyleSheet.create({
  rows: { gap: 2 },
});
