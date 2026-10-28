import { ScrollView, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { PathMark } from '../../components/PathMark';
import { Segmented } from '../../components/Segmented';
import { SessionPanel } from '../../components/SessionPanel';
import { Switch } from '../../components/Switch';
import {
  QUALITY_OPTIONS,
  dialPath,
  pathLine,
  qualityDesc,
  statRows,
  statsLabel,
} from '../../features/session/qualityText';
import { useSetting } from '../../features/settings/store';
import { qualityDisplay } from '../../i18n/zh/qualityDisplay';
import { space, tokens } from '../../theme/tokens';
import type { SheetProps } from './slots';

const text = qualityDisplay;

export function QualitySheet({ ctx, onClose }: SheetProps) {
  const [hud, setHud] = useSetting('session.hud');
  const rec = ctx.rec;
  const display = rec?.displays.find(item => item.id === rec.displayId);
  const stats = rec?.stats ?? null;
  const dialOk = rec?.dial.find(item => item.state === 'ok');
  const rows = statRows(stats);
  const path = dialPath(stats, dialOk);
  const pathText = pathLine(stats, dialOk);

  return (
    <SessionPanel
      closeLabel={text.a11y.close}
      kind="side"
      onClose={onClose}
      subtitle={text.quality.subtitle(ctx.hostName, display?.name ?? null)}
      testID="quality-sheet"
      title={text.screen.title}
    >
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
        style={styles.scroll}
      >
        <View style={styles.section}>
          <SectionHeader title={text.quality.title} />
          <Segmented
            accessibilityLabel={text.quality.a11y}
            onChange={quality => ctx.setQuality(quality)}
            options={QUALITY_OPTIONS.map(value => ({
              value,
              label: text.quality.labels[value],
            }))}
            testID="quality-options"
            value={ctx.quality}
            variant="overlay"
          />
          <AppText color="text2" variant="footnote">
            {qualityDesc(ctx.quality)}
          </AppText>
        </View>

        <View style={styles.section}>
          <SectionHeader title={text.stats.title} />
          <View
            accessible
            accessibilityLabel={statsLabel(rows)}
            style={styles.stats}
            testID="quality-stats"
          >
            {rows.map(row => (
              <View key={row.label} style={styles.stat}>
                <AppText color="text3" variant="caption">
                  {row.label}
                </AppText>
                <AppText mono variant="mono">
                  {row.value}
                </AppText>
              </View>
            ))}
          </View>
          <View style={styles.pathRow}>
            <PathMark kind={path} />
            <AppText color="text2" style={styles.pathText} variant="footnote">
              {pathText}
            </AppText>
          </View>
        </View>

        <ListGroup>
          <ListRow
            right={
              <Switch
                accessibilityLabel={text.hud.label}
                onValueChange={setHud}
                value={hud}
              />
            }
            title={text.hud.label}
          />
        </ListGroup>

        <ListGroup>
          <ListRow
            accessibilityLabel={text.actions.disconnect}
            left={<Icon color="bad" name="power" size={20} />}
            onPress={ctx.disconnect}
            tone="danger"
            title={text.actions.disconnect}
          />
        </ListGroup>
      </ScrollView>
    </SessionPanel>
  );
}

const styles = StyleSheet.create({
  content: {
    gap: space.md,
    paddingBottom: space.lg,
  },
  scroll: {
    flexShrink: 1,
  },
  section: {
    gap: space.sm,
  },
  stats: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    rowGap: space.sm,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    borderRadius: 12,
    backgroundColor: tokens.card,
    padding: space.md,
  },
  stat: {
    width: '50%',
    gap: 2,
    paddingRight: space.sm,
  },
  pathRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  pathText: {
    flexShrink: 1,
  },
});

export default QualitySheet;
