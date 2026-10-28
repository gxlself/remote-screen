import { useCallback, useEffect, useRef, useState } from 'react';
import { PixelRatio, ScrollView, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { PathLabel } from '../../components/PathMark';
import { StatusChip } from '../../components/StatusChip';
import { Switch } from '../../components/Switch';
import { useNetwork } from '../../features/events/EventsProvider';
import { formatHm } from '../../features/format/time';
import { useDirectory } from '../../features/data/queries';
import { useAuthName } from '../../features/lock/authName';
import { ipInCidr } from '../../features/network/cidr';
import { useSetting } from '../../features/settings/store';
import { toggleSensitive } from '../../features/settings/schema';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { common } from '../../i18n/zh/common';
import { pairWaiting } from '../../i18n/zh/pairWaiting';
import type { Address, NetPath, Permission } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';

const copy = pairWaiting.pairDone;

function permissionLabels(permission: Permission): string[] {
  switch (permission) {
    case 'unattended':
      return [copy.permission.allowControl, copy.permission.unattended];
    case 'control':
      return [copy.permission.allowControl];
    case 'viewOnly':
      return [copy.permission.viewOnly];
  }
}

function hostWordOf(os: string | undefined): string {
  return os === 'macos' ? copy.hostWords.mac : copy.hostWords.computer;
}

function osOf(os: string | undefined): 'macos' | 'windows' | null {
  return os === 'macos' || os === 'windows' ? os : null;
}

function AddressRow({
  address,
  path,
  latencyMs,
  sameWifi,
  tailnetEnabled,
}: {
  address: Address;
  path: NetPath | undefined;
  latencyMs: number | undefined;
  sameWifi: boolean;
  tailnetEnabled: boolean;
}) {
  if (address.kind === 'tailnet') {
    const tailnetPath =
      path === 'tailnetDirect' || path === 'tailnetDerp' ? path : undefined;
    const base = tailnetEnabled
      ? copy.addresses.tailnetSubtitle
      : copy.addresses.otherSubtitle;
    const subtitle =
      tailnetPath === undefined
        ? base
        : `${base}${copy.addresses.currentSuffix}`;
    return (
      <ListRow
        right={
          tailnetPath === undefined ? undefined : (
            <PathLabel kind={tailnetPath} latencyMs={latencyMs} size="sm" />
          )
        }
        subtitle={subtitle}
        testID="pair-done-address-tailnet"
        title={address.addr}
      />
    );
  }

  if (address.kind === 'lan') {
    return (
      <ListRow
        right={
          sameWifi ? (
            <StatusChip
              label={copy.addresses.sameWifi}
              size="sm"
              tone="neutral"
            />
          ) : undefined
        }
        subtitle={copy.addresses.lanSubtitle}
        testID="pair-done-address-lan"
        title={address.addr}
      />
    );
  }

  return (
    <ListRow
      subtitle={copy.addresses.intranetSubtitle}
      testID="pair-done-address-intranet"
      title={copy.addresses.intranetTitle(address.addr)}
    />
  );
}

export default function PairDoneScreen({
  navigation,
  route,
}: RootScreenProps<'PairDone'>) {
  const insets = useSafeAreaInsets();
  const fontScale = PixelRatio.getFontScale();
  const authenticationName = useAuthName();
  const { directory } = useDirectory();
  const { status: network } = useNetwork(state => ({ status: state.status }));
  const [sensitiveHosts, setSensitiveHosts] = useSetting('lock.sensitiveHosts');
  const [completedAtUs] = useState(() => Date.now() * 1000);
  const seeded = useRef(false);
  const hostId = route.params.hostId;
  const computer = directory?.computers.find(item => item.hostId === hostId);
  const hostName = route.params.hostName || computer?.name || hostId;
  const hostWord = hostWordOf(computer?.os);
  const permission = route.params.permission;
  const path = route.params.path;
  const latencyMs = route.params.latencyMs;
  const tailnetEnabled = useTailnetEnabled();
  const usedTailnet = path === 'tailnetDirect' || path === 'tailnetDerp';
  // With Tailscale off its addresses only show when pairing went over one.
  const addresses = (computer?.addresses ?? []).filter(
    address => tailnetEnabled || usedTailnet || address.kind !== 'tailnet',
  );
  const sensitive = sensitiveHosts.includes(hostId);
  const [sensitiveAtMount] = useState(() => sensitiveHosts.includes(hostId));

  useEffect(() => {
    if (seeded.current) {
      return;
    }
    seeded.current = true;
    if (!sensitiveAtMount) {
      setSensitiveHosts(toggleSensitive(sensitiveHosts, hostId, true));
    }
  }, [hostId, sensitiveAtMount, sensitiveHosts, setSensitiveHosts]);

  const onSensitiveChange = (value: boolean) => {
    setSensitiveHosts(toggleSensitive(sensitiveHosts, hostId, value));
  };

  const backToDevices = useCallback(() => {
    navigation.reset({ index: 0, routes: [{ name: 'MainTabs' }] });
  }, [navigation]);

  // Back (Android, or any other pop) means the same as 回到设备: the screen
  // that started pairing would only show its stale state.
  useEffect(
    () =>
      navigation.addListener('beforeRemove', event => {
        const type = event.data.action.type;
        if (type !== 'GO_BACK' && type !== 'POP' && type !== 'POP_TO_TOP') {
          return;
        }
        event.preventDefault();
        backToDevices();
      }),
    [backToDevices, navigation],
  );

  // Rust reports interface addresses as `ip/prefix`.
  const sameWifi = (address: Address) =>
    address.cidr !== undefined &&
    (network?.ifaces ?? []).some(iface =>
      iface.addrs.some(addr =>
        ipInCidr(addr.split('/')[0], address.cidr ?? ''),
      ),
    );

  const subtitle = [
    computer?.osVersion,
    computer === undefined || computer.hostVersion === ''
      ? undefined
      : `Host ${computer.hostVersion}`,
    `${common.time.today} ${formatHm(completedAtUs)}`,
  ]
    .filter((part): part is string => part !== undefined && part !== '')
    .join(' · ');

  return (
    <View
      style={[styles.screen, { paddingTop: insets.top + 16 }]}
      testID="pair-done-screen"
    >
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: insets.bottom + 24 },
        ]}
        showsVerticalScrollIndicator={false}
      >
        <View style={styles.hero}>
          <View
            accessible={false}
            importantForAccessibility="no-hide-descendants"
            style={styles.check}
            testID="pair-done-check"
          >
            <Icon color="accentText" name="check" size={30} strokeWidth={2.2} />
          </View>
          <AppText
            accessibilityRole="header"
            center
            variant="title"
            weight={700}
            style={styles.title}
          >
            {copy.title(hostName)}
          </AppText>
          <AppText center color="text3" variant="monoSmall">
            {subtitle}
          </AppText>
        </View>

        {permission === undefined ? null : (
          <View style={styles.section} testID="pair-done-permission">
            <SectionHeader title={copy.permissionTitle(hostWord)} />
            <View
              style={[styles.pills, fontScale > 2 ? styles.pillsLarge : null]}
            >
              {permissionLabels(permission).map((label, index) => (
                <View
                  key={label}
                  style={[styles.pill, index > 0 ? styles.pillAccent : null]}
                >
                  <AppText
                    color={index > 0 ? 'good' : 'text'}
                    variant="caption"
                    weight={600}
                  >
                    {label}
                  </AppText>
                </View>
              ))}
            </View>
            <AppText color="text2" variant="footnote">
              {permission === 'unattended'
                ? copy.permissionNote.unattended(hostWord, osOf(computer?.os))
                : copy.permissionNote.other(hostWord)}
            </AppText>
          </View>
        )}

        <View style={styles.section} testID="pair-done-addresses">
          <SectionHeader
            right={copy.addresses.count(addresses.length)}
            title={copy.addresses.title}
          />
          <ListGroup>
            {addresses.map(address => (
              <AddressRow
                key={`${address.kind}:${address.addr}`}
                address={address}
                latencyMs={latencyMs}
                path={path}
                sameWifi={sameWifi(address)}
                tailnetEnabled={tailnetEnabled}
              />
            ))}
          </ListGroup>
          {addresses.length < 2 ? null : (
            <AppText color="text3" variant="caption">
              {copy.addresses.note}
            </AppText>
          )}
        </View>

        <ListGroup>
          <ListRow
            right={
              <Switch
                accessibilityLabel={copy.sensitive.title(authenticationName)}
                onValueChange={onSensitiveChange}
                testID="pair-done-sensitive"
                value={sensitive}
              />
            }
            subtitle={copy.sensitive.subtitle}
            title={copy.sensitive.title(authenticationName)}
          />
        </ListGroup>

        <View style={styles.actions}>
          <Button
            fullWidth
            onPress={() =>
              navigation.reset({
                index: 1,
                routes: [
                  { name: 'MainTabs' },
                  { name: 'Session', params: { hostId } },
                ],
              })
            }
            title={copy.connectNow}
            variant="primaryLarge"
          />
          <Button
            fullWidth
            onPress={backToDevices}
            title={copy.backToDevices}
            variant="secondary"
          />
        </View>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: tokens.bg },
  content: { paddingHorizontal: 20, gap: 24 },
  hero: { alignItems: 'center', gap: 8, paddingTop: 12 },
  check: {
    width: 64,
    height: 64,
    borderRadius: 32,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.good,
  },
  title: { fontSize: 24, lineHeight: 30 },
  section: { gap: 10 },
  pills: { flexDirection: 'row', gap: 8 },
  pill: {
    minHeight: 28,
    paddingHorizontal: 12,
    borderRadius: 14,
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  pillsLarge: { flexWrap: 'wrap' },
  pillAccent: { backgroundColor: tokens.goodSoft },
  actions: { gap: 10 },
});
