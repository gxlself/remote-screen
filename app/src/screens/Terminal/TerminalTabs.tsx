import { useEffect, useRef } from 'react';
import type { ComponentRef } from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { IconButton } from '../../components/IconButton';
import { tabTitle, termStatus } from '../../features/terminal/tabs';
import type { TabsState } from '../../features/terminal/tabs';
import { terminal } from '../../i18n/zh/terminal';
import type { SshServer } from '../../native/rscore/types';
import { isAndroid, press, ripple } from '../../theme/platform';
import { hit, radius, space, tokens } from '../../theme/tokens';
import { terminalTone } from './StatusLine';

export function TerminalTabs(p: {
  state: TabsState;
  servers: SshServer[];
  tmux: boolean;
  onActivate: (key: string) => void;
  onClose: (key: string) => void;
}) {
  const tabHitSlop = (hit.min - 36) / 2;
  const android = isAndroid();
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
  const count = p.state.tabs.length;
  const previousCount = useRef(count);

  // A new tab opens at the end of the row; bring it into view.
  useEffect(() => {
    if (count > previousCount.current) {
      scroll.current?.scrollToEnd({ animated: true });
    }
    previousCount.current = count;
  }, [count]);

  return (
    <ScrollView
      accessibilityLabel={terminal.tabs}
      accessibilityRole="tablist"
      contentContainerStyle={styles.tabs}
      horizontal
      ref={scroll}
      showsHorizontalScrollIndicator={false}
      style={styles.scroll}
      testID="terminal-tabs"
    >
      {p.state.tabs.map(tab => {
        const active = tab.key === p.state.activeKey;
        const name = tabTitle(tab, p.servers, p.state.tabs);
        const status = termStatus(
          tab,
          p.servers.find(server => server.serverId === tab.serverId),
          p.tmux,
        );
        return (
          <View key={tab.key} style={[styles.tab, active && styles.active]}>
            <Pressable
              accessibilityLabel={terminal.tabLabel(
                name,
                terminal.phase[status.phase],
              )}
              accessibilityRole="tab"
              accessibilityState={{ selected: active }}
              android_ripple={
                android
                  ? { color: ripple.neutral, borderless: true }
                  : undefined
              }
              hitSlop={tabHitSlop}
              onPress={() => p.onActivate(tab.key)}
              style={({ pressed }) => [
                styles.label,
                pressed && !android ? styles.pressed : null,
              ]}
              testID={`terminal-tab-${tab.key}`}
            >
              <View
                style={[
                  styles.dot,
                  { backgroundColor: terminalTone[status.tone] },
                ]}
              />
              <AppText
                color={active ? 'text' : 'text2'}
                variant="footnote"
                weight={active ? 600 : 500}
              >
                {name}
              </AppText>
            </Pressable>
            {active ? (
              <IconButton
                accessibilityLabel={terminal.a11y.closeTab(name)}
                icon="close"
                onPress={() => p.onClose(tab.key)}
                size={36}
                variant="plain"
              />
            ) : null}
          </View>
        );
      })}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  scroll: { flex: 1, alignSelf: 'stretch' },
  tabs: { alignItems: 'center', gap: 6 },
  tab: {
    minHeight: 36,
    borderRadius: radius.thumb,
    paddingHorizontal: space.md,
    flexDirection: 'row',
    alignItems: 'center',
  },
  active: { backgroundColor: tokens.raised },
  label: { minHeight: 36, flexDirection: 'row', alignItems: 'center', gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  pressed: { opacity: press.opacity },
});
