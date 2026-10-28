import { Platform, StyleSheet, View } from 'react-native';
import { useState } from 'react';
import { showToast } from '../../../components/ToastHost';
import { useHostPrefs, useSetting } from '../../../features/settings/store';
import { judgeNetwork } from '../../../features/network/judge';
import { useNetwork } from '../../../features/network/store';
import { ControlUrlSheet } from '../../../features/tailnet/ControlUrlSheet';
import { useTailnetEnabled } from '../../../features/tailnet/enabled';
import { useEmbeddedSwitch } from '../../../features/tailnet/useEmbeddedSwitch';
import { useHostRoutes } from '../../../features/directory/pathStore';
import { formatMs } from '../../../features/format/latency';
import { common } from '../../../i18n/zh/common';
import { errBoards1 } from '../../../i18n/zh/errBoards1';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, NoteBox, StatusRow, summaryText } from '../parts';

export default function VpnBoard({
  params,
  code,
  host,
  hostName,
  actions,
}: BoardProps) {
  const copy = errBoards1.vpn;
  const sw = useEmbeddedSwitch();
  // With Tailscale off the board is only about the company VPN.
  const tailnetEnabled = useTailnetEnabled();
  const embeddedAvailable = tailnetEnabled && sw.available === true;
  const [urlSheet, setUrlSheet] = useState(false);
  const network = useNetwork(state => state.status);
  const [networks] = useSetting('networks');
  const routes = useHostRoutes(params.hostId);
  const [prefs, setPrefs] = useHostPrefs(params.hostId);
  const verdict = judgeNetwork(network, networks);
  const intranetAddresses =
    host?.addresses.filter(address => address.kind === 'intranet') ?? [];
  const tailnetAddresses = tailnetEnabled
    ? host?.addresses.filter(address => address.kind === 'tailnet') ?? []
    : [];
  const lanAddresses =
    host?.addresses.filter(address => address.kind === 'lan') ?? [];
  const intranetAddress = intranetAddresses[0]?.addr;
  const tailnetAddress = tailnetAddresses[0]?.addr;
  const derpRoute = Object.values(routes).find(
    route => route.path === 'tailnetDerp' && route.outcome === 'ok',
  );
  const tailnetUp = verdict.tailnet === 'up';
  const tailnetValue = !tailnetUp
    ? copy.tailnetOff
    : derpRoute === undefined
    ? copy.tailnetReady
    : copy.relay(formatMs(derpRoute.ms));
  const titleAddress = intranetAddress ?? hostName;
  // Builds without the in-app tailnet do not mention it.
  const notes = embeddedAvailable ? [copy.note] : [];
  const illustrationRows = [
    ...intranetAddresses.map(address => ({
      label: copy.intranetLabel(address.addr),
      value: copy.intranetValue,
    })),
    ...tailnetAddresses.map(address => ({
      label: copy.tailnetLabel(address.addr),
      value: tailnetValue,
    })),
    ...(lanAddresses.length === 0
      ? []
      : [{ label: copy.lanLabel, value: copy.lanValue }]),
  ];
  const desc =
    (tailnetAddress === undefined
      ? copy.desc
      : derpRoute === undefined
      ? `${copy.desc}\n${copy.tailnetDesc}`
      : `${copy.desc}\n${copy.tailnetRelayDesc}`) +
    (Platform.OS === 'android' && tailnetEnabled ? `\n${copy.descAndroid}` : '');

  return (
    <>
      <ErrorFrame
        back={{ label: common.back, onPress: actions.back }}
        code={code}
        desc={desc}
        illustrationSummary={summaryText(illustrationRows, notes)}
        illustration={
          <MockWindow dots={false} title={copy.windowTitle(hostName)}>
            <View style={styles.rows}>
              {intranetAddresses.map(address => (
                <StatusRow
                  key={`intranet-${address.addr}`}
                  label={copy.intranetLabel(address.addr)}
                  tone="bad"
                  value={copy.intranetValue}
                />
              ))}
              {tailnetAddresses.map(address => (
                <StatusRow
                  key={`tailnet-${address.addr}`}
                  label={copy.tailnetLabel(address.addr)}
                  tone={
                    !tailnetUp
                      ? 'muted'
                      : derpRoute === undefined
                      ? 'good'
                      : 'relay'
                  }
                  value={tailnetValue}
                />
              ))}
              {lanAddresses.length === 0 ? null : (
                <StatusRow
                  label={copy.lanLabel}
                  tone="muted"
                  value={copy.lanValue}
                />
              )}
            </View>
            {notes.map(note => (
              <NoteBox key={note} text={note} />
            ))}
          </MockWindow>
        }
        kicker={copy.kicker}
        more={
          // V54: shown only when it can work; once switched, connect again.
          embeddedAvailable
            ? [
                {
                  label: copy.appConnect,
                  onPress: () => {
                    sw.switchToEmbedded()
                      .then(result => {
                        if (result === 'needsControlUrl') {
                          setUrlSheet(true);
                        } else if (result === 'switched') {
                          actions.retry();
                        }
                      })
                      .catch(() => undefined);
                  },
                },
              ]
            : []
        }
        onCopyCode={actions.copyCode}
        primary={{
          label: copy.retry,
          onPress: () => actions.retry(),
        }}
        secondary={
          tailnetAddress === undefined
            ? undefined
            : {
                label: copy.useTailnet,
                onPress: () => {
                  // V53: the choice sticks for this computer; say so and
                  // offer to take it back.
                  const previous = prefs.routePolicy;
                  setPrefs({ routePolicy: 'tailnet' });
                  showToast({
                    message: copy.useTailnetSaved,
                    action: {
                      label: copy.undo,
                      onPress: () => setPrefs({ routePolicy: previous }),
                    },
                  });
                  actions.retry();
                },
              }
        }
        title={copy.title(titleAddress)}
      />
      {urlSheet ? (
        <ControlUrlSheet
          onClose={() => setUrlSheet(false)}
          onDone={result => {
            setUrlSheet(false);
            if (result === 'switched') {
              actions.retry();
            }
          }}
        />
      ) : null}
    </>
  );
}

const styles = StyleSheet.create({
  rows: { gap: 2 },
});
