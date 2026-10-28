import type { JSX } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { BottomSheet } from '../../components/BottomSheet';
import { IconButton } from '../../components/IconButton';
import { Icon } from '../../components/icons/Icon';
import type { IconName } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { switcher as copy } from '../../i18n/zh/switcher';
import { windows as windowCopy } from '../../i18n/zh/windows';
import { isAndroid, press, ripple } from '../../theme/platform';
import { tokens } from '../../theme/tokens';
import { sessionCount, showsManyNote } from './switcher';
import type { SwitcherItem, SwitcherList } from './switcher';

const ICONS: Record<SwitcherItem['kind'], IconName> = {
  host: 'computer',
  vnc: 'laptop',
  rdp: 'displays',
  ssh: 'terminal',
};

export type SessionSwitcherProps = {
  list: SwitcherList;
  currentId: string | null;
  /** iPad: show ⌃⌥⌘1…9 next to the first nine screens. */
  shortcuts?: boolean;
  onSelect(item: SwitcherItem): void;
  /** 连接其他电脑. */
  onOther(): void;
  onClose(): void;
  /** iPad: sessions on screen in another window. */
  elsewhereIds?: readonly string[];
  /** iPad: 在新窗口打开; absent where windows are not available. */
  onNewWindow?(item: SwitcherItem): void;
  /** The current session can move to a window of its own (main window). */
  currentNewWindow?: boolean;
};

function Row({
  item,
  current,
  number,
  elsewhere = false,
  onSelect,
  onNewWindow,
}: {
  item: SwitcherItem;
  current: boolean;
  /** 1…9 when a shortcut reaches it. */
  number: number | null;
  /** Shown in another window: a tap brings that window forward. */
  elsewhere?: boolean;
  onSelect(item: SwitcherItem): void;
  /** 在新窗口打开, next to the row. */
  onNewWindow?(item: SwitcherItem): void;
}) {
  const android = isAndroid();
  const kind = copy.kind[item.kind];
  const status = copy.status[item.status];
  let right: JSX.Element | undefined;
  if (elsewhere) {
    right = (
      <AppText
        color="text2"
        testID={`switcher-elsewhere-${item.id}`}
        variant="footnote"
      >
        {windowCopy.elsewhere}
      </AppText>
    );
  } else if (current) {
    right = (
      <AppText color="accent" variant="footnote" weight={500}>
        {copy.current}
      </AppText>
    );
  } else if (number !== null) {
    right = (
      <AppText color="text3" mono variant="monoSmall">
        {copy.shortcut(number)}
      </AppText>
    );
  }
  const row = (
    <Pressable
      accessibilityHint={
        elsewhere
          ? windowCopy.a11y.elsewhere
          : number === null
          ? undefined
          : copy.a11y.shortcut(number)
      }
      accessibilityLabel={copy.a11y.item(item.name, kind, status, current)}
      accessibilityRole="button"
      accessibilityState={{ selected: current }}
      android_ripple={
        android ? { color: ripple.neutral, foreground: true } : undefined
      }
      onPress={() => onSelect(item)}
      style={({ pressed }) => [
        onNewWindow === undefined && current ? styles.current : null,
        onNewWindow === undefined ? null : styles.fill,
        pressed && !android ? styles.pressed : null,
      ]}
      testID={`switcher-item-${item.id}`}
    >
      <ListRow
        left={
          <Icon
            color={current ? 'accent' : 'text2'}
            name={ICONS[item.kind]}
            size={20}
          />
        }
        right={right}
        subtitle={`${kind} · ${status}`}
        title={item.name}
      />
    </Pressable>
  );
  if (onNewWindow === undefined) {
    return row;
  }
  // A sibling, not inside the row: VoiceOver reaches both buttons.
  return (
    <View style={[styles.split, current ? styles.current : null]}>
      {row}
      <IconButton
        accessibilityLabel={windowCopy.a11y.openNew(item.name)}
        icon="external"
        onPress={() => onNewWindow(item)}
        testID={`switcher-new-window-${item.id}`}
        variant="plain"
      />
    </View>
  );
}

/**
 * Every open session; tapping one folds the current session away and opens
 * it full screen.
 *
 * Drawn inside the session page like its other sheets, not in a React Native
 * `Modal`: a Modal is a second view controller presented over the session's
 * full-screen one, and on iPad closing it (tapping the current session) left
 * the page stuck under a sheet whose X no longer answered.
 */
export function SessionSwitcher(p: SessionSwitcherProps): JSX.Element {
  const android = isAndroid();
  const count = sessionCount(p.list);
  return (
    <BottomSheet
      onClose={p.onClose}
      scroll
      subtitle={copy.subtitle}
      testID="session-switcher"
      title={copy.title}
    >
      <View style={styles.content}>
        {showsManyNote(p.list) ? (
          <AppText
            color="text3"
            style={styles.note}
            testID="switcher-many"
            variant="footnote"
          >
            {copy.many(count)}
          </AppText>
        ) : null}
        <ListGroup>
          {p.list.screens.map((item, index) => {
            const current = item.id === p.currentId;
            const elsewhere = p.elsewhereIds?.includes(item.id) === true;
            const onNewWindow =
              !elsewhere && (!current || p.currentNewWindow === true)
                ? p.onNewWindow
                : undefined;
            return (
              <Row
                current={current}
                elsewhere={elsewhere}
                item={item}
                key={item.id}
                number={p.shortcuts === true && index < 9 ? index + 1 : null}
                onSelect={p.onSelect}
                {...(onNewWindow === undefined ? {} : { onNewWindow })}
              />
            );
          })}
          <Pressable
            accessibilityHint={copy.otherSubtitle}
            accessibilityLabel={copy.other}
            accessibilityRole="button"
            android_ripple={
              android ? { color: ripple.neutral, foreground: true } : undefined
            }
            onPress={p.onOther}
            style={({ pressed }) =>
              pressed && !android ? styles.pressed : null
            }
            testID="switcher-other"
          >
            <ListRow
              left={<Icon color="accent" name="computerAdd" size={20} />}
              subtitle={copy.otherSubtitle}
              title={copy.other}
            />
          </Pressable>
        </ListGroup>
        {p.list.terminals.length === 0 ? null : (
          <>
            <SectionHeader title={copy.terminals} />
            <ListGroup>
              {p.list.terminals.map(item => (
                <Row
                  current={item.id === p.currentId}
                  item={item}
                  key={item.id}
                  number={null}
                  onSelect={p.onSelect}
                />
              ))}
            </ListGroup>
          </>
        )}
      </View>
    </BottomSheet>
  );
}

const styles = StyleSheet.create({
  content: { gap: 8 },
  note: { paddingHorizontal: 4 },
  current: { backgroundColor: tokens.accentSoft },
  split: { flexDirection: 'row', alignItems: 'center', paddingRight: 4 },
  fill: { flex: 1 },
  pressed: { backgroundColor: press.highlight },
});

export default SessionSwitcher;
