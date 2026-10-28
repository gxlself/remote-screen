import { PixelRatio, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { PathLabel } from '../../components/PathMark';
import { sessions } from '../../i18n/zh/sessions';
import { radius, space, tokens } from '../../theme/tokens';
import type { RemoteSessionItem } from './useSessionsModel';

export type LiveRemoteCardProps = {
  item: RemoteSessionItem;
  onReturn: () => void;
  onDisconnect: () => void;
};

const THUMB_HEIGHT = 128;

export default function LiveRemoteCard({
  item,
  onReturn,
  onDisconnect,
}: LiveRemoteCardProps) {
  return (
    <View style={styles.card} testID={`remote-card-${item.id}`}>
      <Pressable
        accessibilityHint={sessions.remote.thumbHint(
          item.statusLabel,
          item.displayLabel,
        )}
        accessibilityLabel={sessions.remote.returnA11y(item.name)}
        accessibilityRole="button"
        onPress={onReturn}
        style={({ pressed }) => [styles.thumb, pressed ? styles.pressed : null]}
        testID={`remote-thumb-${item.id}`}
      >
        <Icon color="muted" name="computer" size={32} />
        <View style={styles.hudPill}>
          <View
            style={[
              styles.dot,
              {
                backgroundColor: item.statusRelay ? tokens.relay : tokens.good,
              },
            ]}
          />
          <AppText variant="tiny" weight={600}>
            {item.statusLabel}
          </AppText>
          <AppText color="text2" mono variant="monoSmall">
            {item.elapsed}
          </AppText>
        </View>
        {item.displayLabel === null ? null : (
          <AppText color="text2" mono style={styles.display} variant="tiny">
            {item.displayLabel}
          </AppText>
        )}
      </Pressable>

      <View
        style={[
          styles.footer,
          PixelRatio.getFontScale() > 2 ? styles.footerLarge : null,
        ]}
      >
        <View style={styles.identity}>
          <AppText weight={600} style={styles.name}>
            {item.name}
          </AppText>
          {item.kindLabel === undefined ? null : (
            <AppText color="text3" variant="caption">
              {item.kindLabel}
            </AppText>
          )}
          {item.path === null ? null : (
            <View style={styles.stats}>
              <PathLabel
                kind={item.path}
                latencyMs={item.latencyMs ?? undefined}
                size="sm"
              />
              <AppText color="text3" mono variant="monoSmall">
                {`· ${item.fpsLabel ?? '—'}`}
              </AppText>
            </View>
          )}
        </View>
        <View style={styles.actions}>
          <Button
            accessibilityLabel={sessions.remote.disconnectA11y(item.name)}
            onPress={onDisconnect}
            title={sessions.remote.disconnect}
            variant="secondary"
          />
          <Button
            onPress={onReturn}
            title={sessions.remote.returnToSession}
            variant="primary"
          />
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    gap: 10,
    padding: 10,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    borderRadius: 18,
    backgroundColor: tokens.card,
  },
  thumb: {
    height: THUMB_HEIGHT,
    alignItems: 'center',
    justifyContent: 'center',
    overflow: 'hidden',
    borderRadius: 12,
    backgroundColor: tokens.thumbBg,
  },
  pressed: {
    opacity: 0.8,
  },
  hudPill: {
    position: 'absolute',
    top: 8,
    left: 8,
    minHeight: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    borderRadius: radius.pill,
    backgroundColor: tokens.hud,
  },
  dot: {
    width: 7,
    height: 7,
    borderRadius: 3.5,
  },
  display: {
    position: 'absolute',
    right: 10,
    bottom: 8,
  },
  footer: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
  },
  footerLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  identity: {
    flex: 1,
    gap: 2,
  },
  stats: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  name: {
    flexShrink: 1,
    fontSize: 16,
    lineHeight: 22,
  },
  actions: {
    flexDirection: 'row',
    gap: 6,
  },
});
