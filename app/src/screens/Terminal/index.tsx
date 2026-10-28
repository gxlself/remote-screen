import { useState } from 'react';
import { StyleSheet, View, useWindowDimensions } from 'react-native';
import {
  SafeAreaView,
  useSafeAreaInsets,
} from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { IconButton } from '../../components/IconButton';
import AppText from '../../components/AppText';
import { Toast } from '../../components/Toast';
import { useSshServers } from '../../features/data/queries';
import { tabTitle, termStatus } from '../../features/terminal/tabs';
import { useSnippets } from '../../features/terminal/useSnippets';
import { common } from '../../i18n/zh/common';
import { terminal } from '../../i18n/zh/terminal';
import { useTerminalKeyboard } from '../../native/terminal/terminalKeyboard';
import { space } from '../../theme/tokens';
import { NewTerminalSheet } from './NewTerminalSheet';
import { SnippetBar } from './SnippetBar';
import { SnippetsEditor } from './SnippetsEditor';
import { StatusLine } from './StatusLine';
import { TerminalSlot } from './TerminalSlot';
import { TerminalTabs } from './TerminalTabs';
import { useTerminalController } from './useTerminalController';

// A phone on its side keeps the header to one compact row.
const COMPACT_HEIGHT = 480;
const HEADER_HEIGHT = 52;
const COMPACT_HEADER_HEIGHT = 44;

export default function TerminalScreen(props: RootScreenProps<'Terminal'>) {
  const ctx = useTerminalController(props);
  const { data: servers = [] } = useSshServers();
  const { items } = useSnippets();
  const keyboard = useTerminalKeyboard();
  const window = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const compact = window.height < COMPACT_HEIGHT;
  const [newTerminalOpen, setNewTerminalOpen] = useState(false);
  const [snippetsOpen, setSnippetsOpen] = useState(false);
  const active = ctx.activeTab;
  const closingName =
    ctx.closing === null ? '' : tabTitle(ctx.closing, servers, ctx.s.tabs);
  const accessory = (
    <SnippetBar
      connected={ctx.canSend}
      items={items}
      onEdit={() => setSnippetsOpen(true)}
      onSend={ctx.sendSnippet}
      showEdit={!keyboard.visible}
    />
  );
  const headerHeight = compact ? COMPACT_HEADER_HEIGHT : HEADER_HEIGHT;

  return (
    <SafeAreaView edges={['top', 'left', 'right']} style={styles.root}>
      <View style={[styles.header, { minHeight: headerHeight }]}>
        <IconButton
          accessibilityLabel={common.a11y.back}
          icon="chevronLeft"
          onPress={() => props.navigation.goBack()}
          variant="plain"
        />
        <TerminalTabs
          onActivate={ctx.activate}
          onClose={ctx.requestClose}
          servers={servers}
          state={ctx.s}
          tmux={ctx.tmux}
        />
        <IconButton
          accessibilityLabel={terminal.a11y.paste}
          disabled={!ctx.canSend}
          icon="paste"
          onPress={ctx.paste}
          size={36}
          testID="terminal-paste"
          variant="plain"
        />
        <IconButton
          accessibilityLabel={terminal.a11y.newTerminal}
          icon="plus"
          onPress={() => setNewTerminalOpen(true)}
        />
      </View>
      {active === null ? null : (
        <StatusLine
          onReconnect={() => ctx.reconnect(active.key)}
          status={termStatus(
            active,
            servers.find(server => server.serverId === active.serverId),
            ctx.tmux,
          )}
        />
      )}
      {ctx.unlocked ? (
        <View style={styles.terminals}>
          {ctx.s.tabs.map(tab => {
            const isActive = tab.key === ctx.s.activeKey;
            return (
              <View
                accessibilityElementsHidden={!isActive}
                importantForAccessibility={
                  isActive ? 'auto' : 'no-hide-descendants'
                }
                key={tab.key}
                pointerEvents={isActive ? 'auto' : 'none'}
                style={[styles.pane, isActive ? null : styles.hiddenPane]}
              >
                <TerminalSlot
                  accessibilityLabel={terminal.output}
                  active={isActive}
                  fontSize={ctx.fontSize}
                  sshId={tab.sshId}
                  theme="night"
                  {...(isActive
                    ? {
                        ref: ctx.handleRef,
                        accessory,
                        onCopied: ctx.showCopied,
                      }
                    : {})}
                />
              </View>
            );
          })}
          {ctx.s.tabs.length === 0 ? (
            <>
              <TerminalSlot
                accessibilityLabel={terminal.output}
                accessory={accessory}
                active
                fontSize={ctx.fontSize}
                ref={ctx.handleRef}
                sshId={null}
                theme="night"
              />
              <View
                pointerEvents="box-none"
                style={styles.empty}
                testID="terminal-empty"
              >
                <AppText accessibilityRole="header" center variant="headline">
                  {terminal.empty.title}
                </AppText>
                <AppText center color="text2" variant="body">
                  {terminal.empty.message}
                </AppText>
                <Button
                  align="center"
                  onPress={() => setNewTerminalOpen(true)}
                  testID="terminal-empty-new"
                  title={terminal.newTerminal}
                  variant="primary"
                />
              </View>
            </>
          ) : null}
        </View>
      ) : (
        <View style={styles.terminals} testID="terminal-locked" />
      )}
      <Toast
        // Under the header, clear of the key bar and the system keyboard.
        offset={insets.top + headerHeight + space.xs}
        onHide={ctx.hideToast}
        position="top"
        toast={ctx.toast}
      />
      <Dialog
        actions={[
          {
            label: terminal.closeDialog.confirm,
            onPress: ctx.confirmClose,
            testID: 'terminal-close-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: ctx.cancelClose,
            variant: 'secondary',
          },
        ]}
        message={
          ctx.tmux
            ? terminal.closeDialog.messageTmux
            : terminal.closeDialog.message
        }
        onDismiss={ctx.cancelClose}
        testID="terminal-close-dialog"
        title={terminal.closeDialog.title(closingName)}
        visible={ctx.closing !== null}
      />
      {newTerminalOpen ? (
        <NewTerminalSheet
          onAddServer={() => props.navigation.navigate('AddServer')}
          onClose={() => setNewTerminalOpen(false)}
          onOpen={ctx.openNew}
          openServerIds={ctx.s.tabs.map(tab => tab.serverId)}
        />
      ) : null}
      {snippetsOpen ? (
        <SnippetsEditor onClose={() => setSnippetsOpen(false)} />
      ) : null}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: '#0B0E10' },
  terminals: { flex: 1 },
  pane: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0 },
  hiddenPane: { opacity: 0 },
  header: {
    paddingHorizontal: space.md,
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.xs,
  },
  empty: {
    position: 'absolute',
    top: 0,
    right: 0,
    left: 0,
    bottom: 160,
    alignItems: 'center',
    justifyContent: 'center',
    gap: space.sm,
    paddingHorizontal: space.gutter,
  },
});
