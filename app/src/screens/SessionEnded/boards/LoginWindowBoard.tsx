import { useMemo, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';
import AppText from '../../../components/AppText';
import { Dialog } from '../../../components/Dialog';
import { Icon } from '../../../components/icons/Icon';
import { useHostPath } from '../../../features/events/EventsProvider';
import { ipInCidr } from '../../../features/network/cidr';
import { localHostAddrs } from '../../../features/network/judge';
import { formatHm } from '../../../features/format/time';
import { formatMs } from '../../../features/format/latency';
import { useNetwork } from '../../../features/network/store';
import { useTailnetEnabled } from '../../../features/tailnet/enabled';
import { common } from '../../../i18n/zh/common';
import { errBoards3 } from '../../../i18n/zh/errBoards3';
import { tokens, radius } from '../../../theme/tokens';
import type { BoardProps, EndAction } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { CodeBlock, MockWindow, summaryText } from '../parts';

const copy = errBoards3;

function dateTimeLabel(nowUs: number): string {
  const date = new Date(nowUs / 1000);
  const monthDay = common.time.monthDay(date.getMonth() + 1, date.getDate());
  return `${monthDay} ${common.time.weekdays[date.getDay()]} ${formatHm(
    nowUs,
  )}`;
}

function lanAddress(host: BoardProps['host']): string | null {
  if (host === undefined) {
    return null;
  }
  const lan = host.addresses.find(address => address.kind === 'lan');
  return lan?.addr ?? null;
}

function inHostLan(host: BoardProps['host'], addrs: string[]): boolean {
  if (host === undefined) {
    return false;
  }
  const cidr = host.addresses.find(address => address.kind === 'lan')?.cidr;
  if (cidr === undefined) {
    return false;
  }
  return addrs.some(addr => ipInCidr(addr, cidr));
}

function StageBackground({ dim }: { dim: boolean }) {
  return (
    <Svg
      height="100%"
      style={StyleSheet.absoluteFill}
      viewBox="0 0 240 220"
      width="100%"
    >
      <Defs>
        <LinearGradient id="loginStage" x1="0" x2="0" y1="0" y2="1">
          <Stop offset="0" stopColor={dim ? '#080B0E' : '#16202B'} />
          <Stop offset="1" stopColor={dim ? '#040608' : '#0A0E12'} />
        </LinearGradient>
      </Defs>
      <Rect fill="url(#loginStage)" height="220" width="240" />
    </Svg>
  );
}

function loginIllustration(hostName: string, pillText: string, kicker: string) {
  return (
    <MockWindow dots={false} title={kicker}>
      <View style={styles.loginStage}>
        <StageBackground dim={false} />
        <AppText color="text3" variant="caption">
          {dateTimeLabel(Date.now() * 1000)}
        </AppText>
        <View style={styles.avatar}>
          <Icon color="accent" name="user" size={22} />
        </View>
        <AppText weight={600}>{hostName}</AppText>
        <View style={styles.fakeInput}>
          <AppText color="text3" variant="caption">
            {copy.login.illustration.password}
          </AppText>
        </View>
        <View style={styles.pill}>
          <AppText color="text2" style={styles.pillText}>
            {pillText}
          </AppText>
        </View>
      </View>
      <AppText center color="text3" style={styles.illustrationFooter}>
        {copy.login.illustration.footerLogin}
      </AppText>
    </MockWindow>
  );
}

function offlineText(lan: string | null, tailnetEnabled: boolean): string {
  const ill = copy.filevault.illustration;
  if (lan === null) {
    return tailnetEnabled ? ill.offlineUnknown : ill.offlineUnknownLan;
  }
  return tailnetEnabled ? ill.offline(lan) : ill.offlineLan(lan);
}

function filevaultIllustration(
  host: BoardProps['host'],
  tailnetEnabled: boolean,
) {
  const lan = lanAddress(host);
  return (
    <MockWindow dots={false} title={copy.filevault.kicker}>
      <View style={[styles.loginStage, styles.loginStageDim]}>
        <StageBackground dim />
        <AppText color="text3" variant="caption">
          {dateTimeLabel(Date.now() * 1000)}
        </AppText>
        <View style={styles.avatar}>
          <Icon color="accent" name="user" size={22} />
        </View>
        <View style={styles.fakeInput}>
          <AppText color="text3" variant="caption">
            {copy.login.illustration.password}
          </AppText>
        </View>
        <AppText center color="text3" variant="caption">
          {copy.filevault.illustration.passwordHint}
        </AppText>
        {lan === null ? null : (
          <View style={styles.pill}>
            <AppText color="text2" style={styles.pillText}>
              {offlineText(lan, tailnetEnabled)}
            </AppText>
          </View>
        )}
      </View>
      <AppText center color="text3" style={styles.illustrationFooter}>
        {copy.filevault.illustration.footer}
      </AppText>
    </MockWindow>
  );
}

function Login190Board({
  host,
  hostName,
  os,
  code,
  actions,
  params,
}: BoardProps) {
  // Nothing tells the app when someone logs in, so retrying is manual.
  const hostPath = useHostPath(params.hostId);
  // With Tailscale off the board says the computer is online, not Tailscale.
  const tailnetEnabled = useTailnetEnabled();
  const node = host?.node ?? hostName;
  const pillText = tailnetEnabled
    ? copy.login.illustration.online(node)
    : copy.login.illustration.onlineLan(node);

  const kicker =
    os === 'macos' ? copy.login.kicker.mac : copy.login.kicker.windows;
  const extra =
    hostPath === undefined ? undefined : (
      <AppText color="text2" style={styles.extra}>
        {(tailnetEnabled ? copy.login.extra : copy.login.extraLan)(
          os,
          formatMs(hostPath.latencyMs),
        )}
      </AppText>
    );

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={(tailnetEnabled ? copy.login.desc : copy.login.descLan)(os)}
      illustrationSummary={summaryText([], [pillText])}
      extra={extra}
      illustration={loginIllustration(hostName, pillText, kicker)}
      kicker={kicker}
      onCopyCode={actions.copyCode}
      primary={{
        label: copy.login.action.login(os),
        onPress: () => {
          actions.openScreenSharing().catch(() => undefined);
        },
      }}
      secondary={{
        label: copy.login.action.retry,
        onPress: () => actions.retry(),
      }}
      title={copy.login.title(hostName)}
    />
  );
}

function Login191Board({ host, hostName, os, code, actions }: BoardProps) {
  const network = useNetwork(state => state.status);
  const tailnetEnabled = useTailnetEnabled();
  const [dialogOpen, setDialogOpen] = useState(false);
  const addrs = useMemo(() => localHostAddrs(network), [network]);
  const lan = lanAddress(host);
  const canUnlock = inHostLan(host, addrs) && lan !== null;
  // The app has no account name; the user edits the login before running it.
  const command =
    lan === null ? '' : copy.filevault.dialog.command('user', lan);
  const close = () => {
    setDialogOpen(false);
  };
  const desc =
    (tailnetEnabled ? copy.filevault.desc : copy.filevault.descLan) +
    (canUnlock ? copy.filevault.descSameLan : '');
  const back: EndAction = {
    label:
      os === 'windows'
        ? copy.filevault.action.goUnlockWindows
        : copy.filevault.action.goUnlock,
    onPress: actions.back,
  };

  return (
    <>
      <ErrorFrame
        code={code}
        desc={desc}
        illustrationSummary={summaryText(
          [],
          [
            copy.filevault.illustration.passwordHint,
            offlineText(lan, tailnetEnabled),
          ],
        )}
        illustration={filevaultIllustration(host, tailnetEnabled)}
        kicker={copy.filevault.kicker}
        meta={canUnlock ? copy.filevault.sshMeta : undefined}
        more={[
          {
            label: copy.filevault.action.retry,
            onPress: () => actions.retry(),
          },
        ]}
        onCopyCode={actions.copyCode}
        primary={
          canUnlock
            ? {
                label: copy.filevault.action.ssh,
                onPress: () => {
                  setDialogOpen(true);
                },
              }
            : undefined
        }
        secondary={back}
        title={copy.filevault.title(hostName)}
      />
      <Dialog
        actions={[
          {
            label: copy.filevault.dialog.copy,
            onPress: () => {
              actions.copyText(command, copy.filevault.dialog.copied);
            },
          },
          { label: common.close, onPress: close, variant: 'secondary' },
        ]}
        message={copy.filevault.dialog.body}
        onDismiss={close}
        testID="filevault-ssh-dialog"
        title={copy.filevault.dialog.title}
        visible={dialogOpen}
      >
        <CodeBlock text={command} />
      </Dialog>
    </>
  );
}

export default function LoginWindowBoard(props: BoardProps) {
  return props.code === 191 ? (
    <Login191Board {...props} />
  ) : (
    <Login190Board {...props} />
  );
}

const styles = StyleSheet.create({
  loginStage: {
    alignItems: 'center',
    gap: 8,
    overflow: 'hidden',
    borderRadius: radius.thumb,
    backgroundColor: '#10161D',
    paddingVertical: 20,
    paddingHorizontal: 14,
  },
  loginStageDim: {
    backgroundColor: '#0A0E12',
  },
  avatar: {
    width: 48,
    height: 48,
    borderRadius: 24,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
  fakeInput: {
    alignSelf: 'stretch',
    minHeight: 32,
    justifyContent: 'center',
    borderRadius: 8,
    borderWidth: 1,
    borderColor: tokens.inputLine,
    backgroundColor: tokens.raised,
    paddingHorizontal: 10,
  },
  pill: {
    position: 'absolute',
    bottom: 8,
    left: 8,
    borderRadius: radius.pill,
    backgroundColor: tokens.raised,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  pillText: {
    fontSize: 10,
    lineHeight: 14,
  },
  illustrationFooter: {
    fontSize: 11,
    lineHeight: 14,
  },
  extra: {
    fontSize: 12,
    lineHeight: 17,
  },
});
