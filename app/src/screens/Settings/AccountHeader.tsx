import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { parseIpv4 } from '../../features/network/cidr';
import { judgeNetwork, localHostAddrs } from '../../features/network/judge';
import { useNetwork } from '../../features/network/store';
import { defaultDeviceName } from '../../features/onboarding/discover';
import { useSetting } from '../../features/settings/store';
import { hostKind } from '../../features/ssh/serverForm';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { settings as copy } from '../../i18n/zh/settings';
import { usePlatform } from '../../native/platform/PlatformContext';
import { tokens } from '../../theme/tokens';

export function AccountHeader() {
  const platform = usePlatform();
  const networkStatus = useNetwork(state => state.status);
  const [networks] = useSetting('networks');
  const verdict = judgeNetwork(networkStatus, networks);
  const tailnetEnabled = useTailnetEnabled();
  // There is no account: TailnetStatus carries no login name, so the header
  // names this phone instead.
  const name = defaultDeviceName(null, platform.deviceInfo().model);
  // With Tailscale off the phone's own LAN / VPN address stands in, if any.
  const localAddr = tailnetEnabled
    ? null
    : localHostAddrs(networkStatus).find(
        addr => parseIpv4(addr) !== null && hostKind(addr) === 'private',
      ) ?? null;
  const subtitle = !tailnetEnabled
    ? localAddr === null
      ? undefined
      : copy.account.local(localAddr)
    : verdict.tailnetAddr === null
    ? copy.account.tailnetOff
    : copy.account.tailnetOn(verdict.tailnetAddr);

  return (
    <View style={styles.root}>
      <AppText accessibilityRole="header" variant="largeTitle">
        {copy.title}
      </AppText>
      <ListGroup>
        <ListRow
          accessibilityLabel={copy.account.avatarA11y(name)}
          left={
            <View style={styles.avatar}>
              <Icon color="accent" name="user" size={20} />
            </View>
          }
          subtitle={subtitle}
          title={name}
        />
      </ListGroup>
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    gap: 12,
  },
  avatar: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
});

export default AccountHeader;
