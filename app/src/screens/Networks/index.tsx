import { useMemo, useState } from 'react';
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { useDirectory } from '../../features/data/queries';
import {
  networkState,
  suggestFromHosts,
} from '../../features/network/networks';
import { judgeNetwork } from '../../features/network/judge';
import { useNetwork } from '../../features/network/store';
import { useNetworks } from '../../features/network/useNetworks';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { common } from '../../i18n/zh/common';
import { networks as copy } from '../../i18n/zh/networks';
import { isAndroid, press } from '../../theme/platform';
import type { NetworkEntry } from '../../features/settings/schema';
import { hit, space, tokens } from '../../theme/tokens';
import NetworkEditor from './NetworkEditor';

type EditorState = {
  entry: NetworkEntry | null;
  cidrs: string;
};

export default function NetworksScreen({
  navigation,
}: RootScreenProps<'Networks'>) {
  const insets = useSafeAreaInsets();
  const tailnetEnabled = useTailnetEnabled();
  const { items: networks, isLoading } = useNetworks();
  const networkStatus = useNetwork(state => state.status);
  const { directory } = useDirectory();
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [editorKey, setEditorKey] = useState(0);
  const verdict = judgeNetwork(networkStatus, networks);
  const computers = useMemo(() => directory?.computers ?? [], [directory]);
  const suggestions = useMemo(
    () => suggestFromHosts(computers, networks),
    [computers, networks],
  );

  const openEditor = (entry: NetworkEntry | null, cidrs = '') => {
    setEditorKey(key => key + 1);
    setEditor({ entry, cidrs });
  };

  return (
    <View style={[styles.screen, { paddingTop: insets.top }]}>
      <ScrollView
        contentContainerStyle={styles.content}
        keyboardShouldPersistTaps="handled"
      >
        <Pressable
          accessibilityLabel={copy.screen.back}
          accessibilityRole="button"
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [
            styles.back,
            { minHeight: hit.min },
            pressed && !isAndroid() ? styles.pressed : null,
          ]}
          testID="networks-back"
        >
          <Icon color="accent" name="chevronLeft" size={18} />
          <AppText color="accent" variant="body">
            {copy.screen.backLabel}
          </AppText>
        </Pressable>

        <View style={styles.header}>
          <AppText accessibilityRole="header" variant="title">
            {copy.screen.title}
          </AppText>
          <AppText color="text2" variant="body">
            {tailnetEnabled
              ? copy.screen.description
              : copy.screen.descriptionLan}
          </AppText>
        </View>

        {isLoading ? (
          <ActivityIndicator
            accessibilityLabel={common.loading}
            color={tokens.accent}
            style={styles.loading}
            testID="networks-loading"
          />
        ) : networks.length === 0 ? (
          <EmptyState icon="globe" title={copy.entries.empty} />
        ) : (
          <ListGroup>
            {networks.map(network => (
              <ListRow
                accessibilityLabel={copy.entries.editA11y(network.name)}
                chevron
                key={network.id}
                onPress={() => openEditor(network)}
                subtitle={copy.entries.subtitle(
                  network.cidrs,
                  network.vpnPools,
                )}
                title={network.name}
                value={copy.entries.state[networkState(network, verdict)]}
              />
            ))}
          </ListGroup>
        )}

        <Button
          fullWidth
          onPress={() => openEditor(null)}
          title={copy.entries.add}
        />

        {suggestions.length === 0 ? null : (
          <View style={styles.suggestions}>
            <SectionHeader title={copy.sections.suggestions} />
            <ListGroup>
              {suggestions.map(suggestion => (
                <ListRow
                  key={suggestion.cidr}
                  left={
                    <View
                      accessible
                      accessibilityLabel={`${
                        suggestion.cidr
                      }，${copy.suggestions.subtitle(suggestion.hostNames)}`}
                      style={styles.suggestionText}
                    >
                      <AppText mono variant="mono">
                        {suggestion.cidr}
                      </AppText>
                      <AppText color="text3" variant="caption">
                        {copy.suggestions.subtitle(suggestion.hostNames)}
                      </AppText>
                    </View>
                  }
                  right={
                    <Button
                      accessibilityLabel={copy.suggestions.addA11y(
                        suggestion.cidr,
                      )}
                      onPress={() => openEditor(null, suggestion.cidr)}
                      title={copy.suggestions.add}
                      variant="pill"
                    />
                  }
                  title=""
                />
              ))}
            </ListGroup>
          </View>
        )}
      </ScrollView>

      <NetworkEditor
        entry={editor?.entry ?? null}
        initialCidrs={editor?.cidrs ?? ''}
        key={editorKey}
        onClose={() => setEditor(null)}
        visible={editor !== null}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    gap: space.section,
    paddingHorizontal: space.gutter,
    paddingTop: space.sm,
    paddingBottom: 48,
  },
  back: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    alignSelf: 'flex-start',
  },
  header: {
    gap: space.sm,
  },
  loading: { minHeight: 96 },
  pressed: { opacity: press.opacity },
  suggestions: {
    gap: space.sm,
  },
  suggestionText: {
    gap: 2,
  },
});
