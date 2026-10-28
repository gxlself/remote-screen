import { useState } from 'react';
import { ScrollView, StyleSheet } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { TabScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { ListGroup, ListRow } from '../../components/List';
import { MIN_HOST_VERSION } from '../../features/diagnostics/hostVersion';
import { usePlatform } from '../../native/platform/PlatformContext';
import { settings as copy } from '../../i18n/zh/settings';
import { space, tokens } from '../../theme/tokens';
import AccountHeader from './AccountHeader';
import NetworkSections from './NetworkSections';
import SecuritySection from './SecuritySection';
import SessionDefaultsSection from './SessionDefaultsSection';
import TerminalSection from './TerminalSection';

const TAB_BAR_HEIGHT = 84;

export default function SettingsScreen({
  navigation,
}: TabScreenProps<'Settings'>) {
  const insets = useSafeAreaInsets();
  const platform = usePlatform();
  const [qualitySheetOpen, setQualitySheetOpen] = useState(false);
  const [fontSheetOpen, setFontSheetOpen] = useState(false);
  const [snippetsOpen, setSnippetsOpen] = useState(false);

  return (
    <ScrollView
      contentContainerStyle={[
        styles.content,
        {
          paddingTop: insets.top + 12,
          paddingBottom: insets.bottom + TAB_BAR_HEIGHT + 24,
        },
      ]}
      showsVerticalScrollIndicator={false}
      style={styles.screen}
      testID="settings-scroll"
    >
      <AccountHeader />

      <SecuritySection navigation={navigation} />

      <SessionDefaultsSection
        onCloseQualitySheet={() => setQualitySheetOpen(false)}
        onOpenQualitySheet={() => setQualitySheetOpen(true)}
        qualitySheetOpen={qualitySheetOpen}
      />

      <NetworkSections navigation={navigation} />

      <TerminalSection
        fontSheetOpen={fontSheetOpen}
        onCloseFontSheet={() => setFontSheetOpen(false)}
        onCloseSnippets={() => setSnippetsOpen(false)}
        onOpenFontSheet={() => setFontSheetOpen(true)}
        onOpenSnippets={() => setSnippetsOpen(true)}
        snippetsOpen={snippetsOpen}
      />

      <ListGroup>
        <ListRow
          chevron
          onPress={() => navigation.navigate('Diagnostics')}
          subtitle={copy.diagnostics.subtitle}
          title={copy.diagnostics.title}
        />
        {__DEV__ ? (
          <ListRow
            chevron
            onPress={() => navigation.navigate('DevSelfTest')}
            title={copy.diagnostics.selfTest}
          />
        ) : null}
      </ListGroup>

      <AppText center color="text3" variant="caption">
        {copy.footer(platform.deviceInfo().appVersion, MIN_HOST_VERSION)}
      </AppText>
    </ScrollView>
  );
}

const SETTINGS_MAX_WIDTH = 680;

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  // S31: on iPad keep rows at a readable width instead of edge to edge.
  content: {
    gap: space.section,
    paddingHorizontal: space.gutter,
    width: '100%',
    maxWidth: SETTINGS_MAX_WIDTH,
    alignSelf: 'center',
  },
});
