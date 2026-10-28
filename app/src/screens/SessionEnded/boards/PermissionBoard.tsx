import { StyleSheet, View } from 'react-native';
import AppText from '../../../components/AppText';
import { Switch } from '../../../components/Switch';
import { common } from '../../../i18n/zh/common';
import { errBoards1 } from '../../../i18n/zh/errBoards1';
import { sessionEnded } from '../../../i18n/zh/sessionEnded';
import { tokens } from '../../../theme/tokens';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { ChipPath, MockWindow, NoteBox, summaryText } from '../parts';
import { useTerminalServer } from '../terminalServer';

function PermissionRow(props: {
  label: string;
  value: boolean;
  highlighted?: boolean;
}) {
  return (
    <View style={props.highlighted ? styles.highlightedRow : styles.row}>
      <AppText style={styles.rowLabel}>{props.label}</AppText>
      <Switch
        accessibilityLabel={props.label}
        disabled
        onValueChange={() => undefined}
        value={props.value}
      />
    </View>
  );
}

export default function PermissionBoard({
  params,
  code,
  host,
  hostName,
  os,
  actions,
}: BoardProps) {
  const mac = os === 'macos';
  const copy = errBoards1.permission;
  const permission =
    params.detail?.perm === 'accessibility' ? 'accessibility' : 'screen';
  const permissionCopy = copy[permission];
  const osName = copy.os[os ?? 'unknown'];
  const title = permissionCopy.title(hostName);
  // V58: the terminal only when a listed SSH server reaches this computer.
  const serverId = useTerminalServer(host);
  const desc =
    permissionCopy.desc(mac) + (serverId === null ? '' : copy.terminalDesc);

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={desc}
      illustrationSummary={summaryText([], [copy.window.note(osName)])}
      extra={
        <ChipPath
          accessibilityLabel={copy.pathLabel}
          items={[...permissionCopy.path]}
        />
      }
      illustration={
        <MockWindow title={permissionCopy.windowTitle}>
          <AppText color="text2" style={styles.windowDesc}>
            {permissionCopy.windowDesc}
          </AppText>
          <PermissionRow
            highlighted
            label={copy.window.remoteHost}
            value={false}
          />
          <PermissionRow label={copy.window.meeting} value />
          <PermissionRow label={copy.window.screenshot} value />
          <NoteBox text={copy.window.note(osName)} />
        </MockWindow>
      }
      kicker={copy.kicker}
      meta={
        params.detail?.opened === 'true' ? permissionCopy.meta(mac) : undefined
      }
      more={
        serverId === null
          ? []
          : [
              {
                label: copy.buttons.terminal,
                onPress: () => actions.openTerminal(serverId),
              },
            ]
      }
      onCopyCode={actions.copyCode}
      primary={{
        label: copy.buttons.retry,
        onPress: () => actions.retry(),
      }}
      secondary={{
        label: sessionEnded.useDesktop(os),
        onPress: () => {
          Promise.resolve(actions.openScreenSharing()).catch(() => undefined);
        },
      }}
      title={title}
    />
  );
}

const styles = StyleSheet.create({
  windowDesc: { fontSize: 11.5, lineHeight: 16 },
  row: {
    minHeight: 38,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
  },
  highlightedRow: {
    minHeight: 42,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: tokens.accent,
    paddingHorizontal: 6,
  },
  rowLabel: { fontSize: 11.5, lineHeight: 16 },
});
