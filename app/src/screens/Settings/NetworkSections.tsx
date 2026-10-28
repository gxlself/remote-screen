import { useState } from 'react';
import type { ReactNode } from 'react';
import { ActivityIndicator, Platform, StyleSheet, View } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { Switch } from '../../components/Switch';
import { showToast } from '../../components/ToastHost';
import { judgeNetwork } from '../../features/network/judge';
import { networkState } from '../../features/network/networks';
import { useNetwork } from '../../features/network/store';
import { useManualServers } from '../../features/servers/useManualServers';
import { useSetting, useSettingsStore } from '../../features/settings/store';
import {
  TailscaleApiRow,
  TailscaleApiSheet,
} from '../../features/tailscale/TailscaleApiSheet';
import { ControlUrlSheet } from '../../features/tailnet/ControlUrlSheet';
import { useEmbeddedSwitch } from '../../features/tailnet/useEmbeddedSwitch';
import { useTailnetRoute } from '../../features/tailnet/useTailnetRoute';
import { networks as copy } from '../../i18n/zh/networks';
import { useCore } from '../../native/rscore/CoreContext';
import { space, tokens } from '../../theme/tokens';

type Props = {
  navigation: TabScreenProps<'Settings'>['navigation'];
};

const TAILNET_STATES = {
  up: copy.tailnet.up,
  down: copy.tailnet.down,
  unknown: copy.tailnet.unknown,
} as const;

const NETWORK_STATES = {
  vpn: copy.entries.state.vpn,
  inside: copy.entries.state.inside,
  none: copy.entries.state.none,
} as const;

const TAILNET_COLORS = {
  up: 'good',
  down: 'relay',
  unknown: 'text2',
} as const;

export default function NetworkSections({ navigation }: Props) {
  const { computers } = useManualServers();
  const [tailnetEnabled, setTailnetEnabled] = useSetting('tailnet.enabled');
  const [prewarm, setPrewarm] = useSetting('prewarm.enabled');
  const [preferLocal, setPreferLocal] = useSetting('net.preferLocal');
  const [warnVpnConflict, setWarnVpnConflict] = useSetting(
    'net.warnVpnConflict',
  );
  const [networks] = useSetting('networks');
  const networkStatus = useNetwork(state => state.status);
  const verdict = judgeNetwork(networkStatus, networks);

  const prewarmRow = (
    <ListRow
      right={
        <Switch
          accessibilityHint={copy.prewarm.subtitle}
          accessibilityLabel={copy.prewarm.title}
          onValueChange={setPrewarm}
          value={prewarm}
        />
      }
      subtitle={copy.prewarm.subtitle}
      title={copy.prewarm.title}
    />
  );

  const switchRow = (
    <ListRow
      right={
        <Switch
          accessibilityHint={copy.outside.subtitle}
          accessibilityLabel={copy.outside.title}
          onValueChange={setTailnetEnabled}
          testID="tailnet-enabled-switch"
          value={tailnetEnabled}
        />
      }
      subtitle={copy.outside.subtitle}
      title={copy.outside.title}
    />
  );

  return (
    <View style={styles.sections}>
      {/* Tailscale is opt-in: its whole block exists only while it is on. */}
      {tailnetEnabled ? (
        <TailnetSection prewarmRow={prewarmRow} switchRow={switchRow} />
      ) : (
        <View style={styles.section}>
          <SectionHeader title={copy.sections.access} />
          <ListGroup>
            {switchRow}
            {prewarmRow}
          </ListGroup>
        </View>
      )}

      <View style={styles.section}>
        <SectionHeader title={copy.sections.other} />
        <ListGroup>
          {networks.length === 0 ? (
            <ListRow
              chevron
              onPress={() => navigation.navigate('Networks')}
              title={copy.entries.add}
            />
          ) : (
            networks.map(network => (
              // S14: the row opens the list, not the editor; say so.
              <ListRow
                accessibilityHint={copy.entries.openHint}
                chevron
                key={network.id}
                onPress={() => navigation.navigate('Networks')}
                subtitle={copy.entries.subtitle(
                  network.cidrs,
                  network.vpnPools,
                )}
                title={network.name}
                value={NETWORK_STATES[networkState(network, verdict)]}
              />
            ))
          )}
          <ListRow
            chevron
            onPress={() => navigation.navigate('AddByAddress')}
            title={copy.entries.addByAddress}
            value={copy.entries.count(computers.length)}
          />
          <ListRow
            right={
              <Switch
                accessibilityHint={preferLocalSubtitle(tailnetEnabled)}
                accessibilityLabel={copy.prefs.preferLocal.title}
                onValueChange={setPreferLocal}
                value={preferLocal}
              />
            }
            subtitle={preferLocalSubtitle(tailnetEnabled)}
            title={copy.prefs.preferLocal.title}
          />
          {tailnetEnabled ? (
            <ListRow
              right={
                <Switch
                  accessibilityHint={vpnConflictSubtitle()}
                  accessibilityLabel={copy.prefs.warnVpnConflict.title}
                  onValueChange={setWarnVpnConflict}
                  value={warnVpnConflict}
                />
              }
              subtitle={vpnConflictSubtitle()}
              title={copy.prefs.warnVpnConflict.title}
            />
          ) : null}
        </ListGroup>
        <AppText color="text3" style={styles.footer} variant="caption">
          {tailnetEnabled ? copy.footer : copy.footerLan}
        </AppText>
      </View>
    </View>
  );
}

/** The Tailscale settings, unchanged from before the 外网访问 switch. */
function TailnetSection({
  prewarmRow,
  switchRow,
}: {
  prewarmRow: ReactNode;
  switchRow: ReactNode;
}) {
  const { available, switchToEmbedded, openLogin } = useEmbeddedSwitch();
  const route = useTailnetRoute();
  const networkStatus = useNetwork(state => state.status);
  const [networks] = useSetting('networks');
  const [tailnetMode, setTailnetMode] = useSetting('tailnet.mode');
  const [controlUrl] = useSetting('tailnet.controlUrl');
  const tailscaleApi = useCore().tailscaleApi !== undefined;
  const settingsStore = useSettingsStore();
  // 'auto': the save-only sheet opened by picking 自动 without an address;
  // the mode changes only once an address is saved.
  const [sheet, setSheet] = useState<
    'switch' | 'edit' | 'auto' | 'tailscaleApi' | null
  >(null);
  const [switching, setSwitching] = useState(false);
  const verdict = judgeNetwork(networkStatus, networks);

  const embeddedActive = available === true && route?.active === 'embedded';
  let tailnetLabel: string = TAILNET_STATES[verdict.tailnet];
  let tailnetColor: 'good' | 'relay' | 'text2' =
    TAILNET_COLORS[verdict.tailnet];
  if (embeddedActive) {
    const embeddedState = route.embedded?.state ?? null;
    if (embeddedState === 'running') {
      tailnetLabel = copy.tailnet.embeddedStates.running;
      tailnetColor = 'good';
    } else if (
      embeddedState === 'needsLogin' ||
      embeddedState === 'needsMachineAuth'
    ) {
      tailnetLabel = copy.tailnet.embeddedStates.needsLogin;
      tailnetColor = 'relay';
    } else if (
      embeddedState === 'noState' ||
      embeddedState === 'starting' ||
      embeddedState === null
    ) {
      tailnetLabel = copy.tailnet.embeddedStates.starting;
      tailnetColor = 'text2';
    } else {
      tailnetLabel = copy.tailnet.embeddedStates.down;
      tailnetColor = 'relay';
    }
  }
  const needsLogin =
    embeddedActive &&
    (route.embedded?.state === 'needsLogin' ||
      route.embedded?.state === 'needsMachineAuth');
  // S17: the phone's own Tailscale is off; say where to turn it on.
  const systemDown = !embeddedActive && verdict.tailnet === 'down';
  const controlUrlEmpty = controlUrl == null || controlUrl === '';
  const controlUrlValue = controlUrlEmpty
    ? copy.tailnet.controlUrlEmpty
    : controlUrl.replace(/^https:\/\//, '');
  const modeHint =
    tailnetMode === 'embedded'
      ? copy.tailnet.embeddedHint
      : tailnetMode === 'auto'
      ? copy.tailnet.autoHint
      : copy.tailnet.systemHint;

  const chooseMode = (next: 'system' | 'embedded' | 'auto') => {
    if (next === 'system') {
      setTailnetMode('system');
      return;
    }
    if (next === 'auto') {
      if (controlUrlEmpty) {
        setSheet('auto');
      } else {
        setTailnetMode('auto');
      }
      return;
    }
    setSwitching(true);
    switchToEmbedded()
      .then(result => {
        if (result === 'needsControlUrl') {
          setSheet('switch');
        } else if (result === 'unavailable') {
          showToast({ message: copy.tailnet.switchFailed, tone: 'bad' });
        }
      })
      .catch(() =>
        showToast({ message: copy.tailnet.switchFailed, tone: 'bad' }),
      )
      .finally(() => setSwitching(false));
  };

  // S06: while the probe runs the App-only modes are listed but held, so the
  // row keeps its size; a phone without them shows the one mode as text.
  const modeControl =
    available === false ? null : (
      <Segmented
        accessibilityLabel={copy.tailnet.modeA11y}
        onChange={chooseMode}
        options={(['system', 'embedded', 'auto'] as const).map(value => ({
          value,
          label: copy.tailnet.modes[value],
          disabled: switching || (value !== 'system' && available !== true),
        }))}
        value={available === true ? tailnetMode : 'system'}
      />
    );

  return (
    <>
      <View style={styles.section}>
        <SectionHeader title={copy.sections.access} />
        <ListGroup>
          {switchRow}
          <ListRow
            accessibilityLabel={`${copy.tailnet.title}，${tailnetLabel}`}
            subtitle={systemDown ? copy.tailnet.downHint : undefined}
            title={copy.tailnet.title}
            value={
              <AppText color={tailnetColor} variant="body">
                {tailnetLabel}
              </AppText>
            }
          />
          {modeControl === null ? (
            <ListRow
              title={copy.tailnet.modeTitle}
              value={copy.tailnet.modes.system}
            />
          ) : (
            // S04: the three modes get the full row width under the title.
            <View style={styles.modeStack}>
              <AppText variant="body">{copy.tailnet.modeTitle}</AppText>
              {modeControl}
              {switching ? (
                <ActivityIndicator
                  accessibilityLabel={copy.tailnet.switching}
                  color={tokens.accent}
                  size="small"
                  style={styles.modeSpinner}
                  testID="tailnet-mode-switching"
                />
              ) : null}
            </View>
          )}
          {available === true && tailnetMode !== 'system' ? (
            <ListRow
              chevron
              onPress={() => setSheet('edit')}
              title={copy.tailnet.controlUrlTitle}
              value={controlUrlValue}
            />
          ) : null}
          {available === true && needsLogin ? (
            <ListRow
              chevron
              onPress={() => {
                openLogin()
                  .then(opened => {
                    if (!opened) {
                      showToast({
                        message: copy.tailnet.loginFailed,
                        tone: 'bad',
                      });
                    }
                  })
                  .catch(() => undefined);
              }}
              title={copy.tailnet.loginTitle}
              value={copy.tailnet.loginValue}
            />
          ) : null}
          {tailscaleApi ? (
            <TailscaleApiRow onPress={() => setSheet('tailscaleApi')} />
          ) : null}
          {prewarmRow}
        </ListGroup>
        <AppText color="text3" style={styles.hint} variant="footnote">
          {available === true ? modeHint : copy.tailnet.systemHint}
        </AppText>
      </View>

      {sheet === 'switch' ? (
        <ControlUrlSheet
          onClose={() => setSheet(null)}
          onDone={result => {
            setSheet(null);
            if (result === 'unavailable') {
              showToast({ message: copy.tailnet.switchFailed, tone: 'bad' });
            }
          }}
        />
      ) : null}
      {sheet === 'edit' || sheet === 'auto' ? (
        <ControlUrlSheet
          onClose={() => setSheet(null)}
          onDone={() => {
            const pickedAuto = sheet === 'auto';
            setSheet(null);
            // S10: say what the save did.
            const saved = settingsStore.get('tailnet.controlUrl');
            if (pickedAuto && saved !== null) {
              setTailnetMode('auto');
            }
            showToast({
              message:
                saved === null
                  ? pickedAuto
                    ? copy.tailnet.autoNeedsUrl
                    : copy.tailnet.cleared
                  : copy.tailnet.saved,
              tone: saved === null && pickedAuto ? 'neutral' : 'good',
            });
          }}
          saveOnly
        />
      ) : null}
      {sheet === 'tailscaleApi' ? (
        <TailscaleApiSheet
          onClose={() => setSheet(null)}
          onDone={message => {
            setSheet(null);
            showToast({ message });
          }}
        />
      ) : null}
    </>
  );
}

function preferLocalSubtitle(tailnetEnabled: boolean): string {
  return tailnetEnabled
    ? copy.prefs.preferLocal.subtitle
    : copy.prefs.preferLocal.subtitleLan;
}

function vpnConflictSubtitle(): string {
  return Platform.OS === 'android'
    ? copy.prefs.warnVpnConflict.subtitleAndroid
    : copy.prefs.warnVpnConflict.subtitle;
}

const styles = StyleSheet.create({
  sections: {
    gap: space.section,
  },
  section: {
    gap: space.sm,
  },
  hint: {
    paddingHorizontal: space.sm,
    fontSize: 13,
    lineHeight: 18,
  },
  footer: {
    paddingHorizontal: space.sm,
  },
  modeStack: {
    gap: space.sm,
    paddingHorizontal: 14,
    paddingVertical: 12,
  },
  modeSpinner: {
    position: 'absolute',
    top: 12,
    right: 14,
  },
});
