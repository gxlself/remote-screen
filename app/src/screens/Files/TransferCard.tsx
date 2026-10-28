import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { IconButton } from '../../components/IconButton';
import { PathMark } from '../../components/PathMark';
import type { TransferRowModel } from '../../features/files/transferView';
import { files } from '../../i18n/zh/files';
import { radius, space, tokens } from '../../theme/tokens';

export function TransferCard({
  row,
  variant,
  onAction,
}: {
  row: TransferRowModel;
  variant: 'active' | 'stalled';
  onAction(): void;
}) {
  if (variant === 'stalled') {
    const relay = row.tone === 'relay';
    const bad = row.tone === 'bad';
    const iconColor = relay ? 'relay' : bad ? 'bad' : 'text2';
    return (
      <View
        style={[
          styles.stalledCard,
          relay ? styles.relayCard : null,
          bad ? styles.badCard : null,
        ]}
      >
        <View
          style={[
            styles.iconBox,
            relay ? styles.relayIconBox : null,
            bad ? styles.badIconBox : null,
          ]}
        >
          <Icon color={iconColor} name={row.icon} size={20} />
        </View>
        <View accessible accessibilityLabel={row.a11y} style={styles.copy}>
          <AppText variant="body" weight={500}>
            {row.title}
          </AppText>
          <AppText color="text2" variant="caption">
            {row.route}
          </AppText>
        </View>
        <Button
          accessibilityLabel={
            row.action === 'resume'
              ? files.card.resume(row.title)
              : files.card.retry(row.title)
          }
          onPress={onAction}
          title={
            row.action === 'resume'
              ? files.card.resumeLong
              : files.card.retryShort
          }
          variant="secondary"
        />
      </View>
    );
  }

  return (
    <View style={styles.activeCard}>
      <View style={styles.header}>
        <View style={styles.iconBox}>
          <Icon color="text2" name={row.icon} size={20} />
        </View>
        <View accessible accessibilityLabel={row.a11y} style={styles.copy}>
          <AppText variant="body" weight={500}>
            {row.title}
          </AppText>
          <View style={styles.routeLine}>
            <AppText color="text3" variant="caption">
              {row.route}
            </AppText>
            {row.pathMark === null ? null : (
              <>
                <AppText color="text3" variant="caption">
                  {' · '}
                </AppText>
                <PathMark kind={row.pathMark} />
                {row.pathLabel === null ? null : (
                  <AppText color="text3" variant="caption">
                    {row.pathLabel}
                  </AppText>
                )}
              </>
            )}
          </View>
        </View>
        {row.action === 'pause' ? (
          <IconButton
            accessibilityLabel={files.a11y.pause(row.title)}
            icon="pause"
            onPress={onAction}
            variant="raised"
          />
        ) : row.action === 'resume' ? (
          <Button
            accessibilityLabel={files.card.resume(row.title)}
            onPress={onAction}
            title={files.card.resumeShort}
            variant="secondary"
          />
        ) : null}
      </View>
      <View
        accessible
        accessibilityLabel={files.card.progress(row.title)}
        accessibilityRole="progressbar"
        accessibilityValue={{ min: 0, max: 100, now: row.pct }}
        style={styles.progressTrack}
      >
        <View style={[styles.progressFill, { width: `${row.pct}%` }]} />
      </View>
      <View style={styles.numbers}>
        <AppText color="text2" mono style={styles.monoText} variant="monoSmall">
          {row.progressText}
        </AppText>
        <AppText color="text2" mono style={styles.monoText} variant="monoSmall">
          {row.speedText}
        </AppText>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  activeCard: {
    gap: 10,
    padding: space.md,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  stalledCard: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.md,
    padding: space.md,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  relayCard: {
    borderColor: 'rgba(244, 178, 78, 0.32)',
    backgroundColor: 'rgba(244, 178, 78, 0.07)',
  },
  badCard: {
    borderColor: tokens.bad,
    backgroundColor: tokens.badSoft,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.md,
  },
  iconBox: {
    width: 40,
    height: 40,
    borderRadius: 9,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  relayIconBox: {
    backgroundColor: tokens.relaySoft,
  },
  badIconBox: {
    backgroundColor: tokens.badSoft,
  },
  copy: {
    flex: 1,
    minWidth: 0,
    gap: 2,
  },
  routeLine: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 4,
  },
  progressTrack: {
    height: 4,
    borderRadius: 2,
    overflow: 'hidden',
    backgroundColor: tokens.track,
  },
  progressFill: {
    height: '100%',
    borderRadius: 2,
    backgroundColor: tokens.accent,
  },
  numbers: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  monoText: {
    fontSize: 11.5,
    lineHeight: 16,
  },
});

export default TransferCard;
