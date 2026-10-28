import { ActivityIndicator, Modal, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { BottomSheet } from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { ListGroup, ListRow } from '../../components/List';
import { useSshServers } from '../../features/data/queries';
import { common } from '../../i18n/zh/common';
import { terminal } from '../../i18n/zh/terminal';
import { space, tokens } from '../../theme/tokens';

export function NewTerminalSheet(p: {
  onClose: () => void;
  onOpen: (serverId: string) => void;
  onAddServer: () => void;
  // Servers that already have a tab here.
  openServerIds?: readonly string[];
}) {
  const query = useSshServers();
  const servers = query.data ?? [];
  const failed = query.isError && servers.length === 0;
  return (
    <Modal animationType="none" onRequestClose={p.onClose} transparent visible>
      <BottomSheet
        onClose={p.onClose}
        scroll
        testID="new-terminal-sheet"
        title={terminal.newTerminal}
      >
        {query.isLoading ? (
          <View style={styles.state}>
            <ActivityIndicator
              accessibilityLabel={common.loading}
              color={tokens.accent}
              testID="new-terminal-loading"
            />
          </View>
        ) : failed ? (
          <View style={styles.state} testID="new-terminal-failed">
            <AppText color="text2" variant="body">
              {terminal.newSheet.failed}
            </AppText>
            <Button
              align="center"
              loading={query.isFetching}
              onPress={() => {
                query.refetch().catch(() => undefined);
              }}
              testID="new-terminal-retry"
              title={common.retry}
              variant="secondary"
            />
          </View>
        ) : null}
        <ListGroup>
          {servers.map(server => {
            const subtitle =
              [server.os, terminal.serverAuth[server.auth]]
                .filter(Boolean)
                .join(' · ') + (server.jump ? terminal.via(server.jump) : '');
            const opened = p.openServerIds?.includes(server.serverId) ?? false;
            return (
              <ListRow
                accessibilityHint={
                  opened ? `${terminal.newSheet.opened}，${subtitle}` : subtitle
                }
                accessibilityLabel={server.name}
                key={server.serverId}
                onPress={() => {
                  p.onOpen(server.serverId);
                  p.onClose();
                }}
                subtitle={subtitle}
                title={server.name}
                value={opened ? terminal.newSheet.opened : undefined}
              />
            );
          })}
          <ListRow
            accessibilityLabel={terminal.addServer}
            chevron
            onPress={() => {
              p.onClose();
              p.onAddServer();
            }}
            title={terminal.addServer}
          />
        </ListGroup>
      </BottomSheet>
    </Modal>
  );
}

const styles = StyleSheet.create({
  state: {
    minHeight: 96,
    alignItems: 'center',
    justifyContent: 'center',
    gap: space.sm,
    marginBottom: space.md,
  },
});
