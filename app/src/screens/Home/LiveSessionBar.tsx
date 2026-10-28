import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import type { AdapterRouteParams } from '../../features/adapters/liveStore';
import { formatClock } from '../../features/format/duration';
import type { SessionRecord } from '../../features/session/store';
import {
  adapterStatus,
  remoteStatus,
} from '../../features/sessions/remoteStatus';
import type { AdapterPhase } from '../../native/rscore/types';
import { home } from '../../i18n/zh/home';
import { ipadHome } from '../../i18n/zh/ipadHome';
import { tokens } from '../../theme/tokens';

/** What the bar shows: a live Host session or a folded-away adapter one. */
export type LiveBarSession =
  | (Pick<
      SessionRecord,
      'id' | 'hostId' | 'hostName' | 'startedAt' | 'phase' | 'role' | 'path'
    > & { adapter?: undefined })
  | {
      id: string;
      /** The computer's Host id, or a key no card has for a typed address. */
      hostId: string;
      hostName: string;
      startedAt: number;
      path: null;
      /** Screen Sharing / Remote Desktop (V33). */
      adapter: { phase: AdapterPhase; params: AdapterRouteParams };
    };

export type LiveSessionBarProps = {
  session: LiveBarSession;
  bottom: number;
  onPress: () => void;
};

export function LiveSessionBar({
  session,
  bottom,
  onPress,
}: LiveSessionBarProps) {
  const [nowUs, setNowUs] = useState(() => Date.now() * 1000);

  useEffect(() => {
    setNowUs(Date.now() * 1000);
    const timer = setInterval(() => setNowUs(Date.now() * 1000), 1000);
    return () => clearInterval(timer);
  }, [session.startedAt]);

  const elapsed = Math.max(
    0,
    Math.floor((nowUs - session.startedAt) / 1_000_000),
  );
  // Connecting and reconnecting read as such, not as a green 控制中.
  const status =
    session.adapter === undefined
      ? remoteStatus(session.phase, session.role)
      : adapterStatus(session.adapter.phase);
  const role = status.label;

  return (
    <Pressable
      accessibilityLabel={home.live.a11y(session.hostName)}
      accessibilityRole="button"
      // Keep the legacy label while excluding this non-accessible wrapper from
      // accessibility-tree adapters.
      aria-label=""
      accessible={false}
      onPress={onPress}
      style={[styles.bar, { bottom }]}
      testID="live-session-bar"
    >
      <View style={styles.thumb}>
        <Icon color="muted" name="computer" size={20} />
      </View>
      <View
        accessibilityLabel={ipadHome.a11y.liveSummary(session.hostName, role)}
        accessible
        style={styles.copy}
      >
        <AppText variant="button" weight={600}>
          {session.hostName}
        </AppText>
        <View style={styles.statusRow}>
          <View
            style={[
              styles.dot,
              { backgroundColor: status.relay ? tokens.relay : tokens.good },
            ]}
            testID="live-status-dot"
          />
          <AppText color="text2" variant="caption">
            {role}
          </AppText>
          <AppText color="text2" mono variant="monoSmall" testID="live-clock">
            {formatClock(elapsed)}
          </AppText>
        </View>
      </View>
      <Button
        accessibilityLabel={home.live.a11y(session.hostName)}
        onPress={onPress}
        title={home.live.back}
        variant="pill"
      />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  bar: {
    position: 'absolute',
    left: 12,
    right: 12,
    minHeight: 60,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    borderRadius: 18,
    borderWidth: 1,
    borderColor: '#2E383F',
    backgroundColor: 'rgba(30,37,43,0.94)',
    paddingHorizontal: 8,
  },
  thumb: {
    width: 70,
    height: 44,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.thumbBg,
  },
  copy: {
    flex: 1,
    gap: 2,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
});
