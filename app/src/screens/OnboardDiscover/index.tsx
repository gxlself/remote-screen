import { useEffect, useMemo, useRef } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  PixelRatio,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { Button } from '../../components/Button';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { useDirectory } from '../../features/data/queries';
import {
  discoverTitle,
  groupForDiscover,
  hasComputers,
} from '../../features/onboarding/discover';
import type { DiscoverGroups } from '../../features/onboarding/discover';
import { useManualServers } from '../../features/servers/useManualServers';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { onboardDiscover } from '../../i18n/zh/onboardDiscover';
import { space, tokens } from '../../theme/tokens';
import StepHeader from '../_onboarding/StepHeader';
import { DiscoverRow } from './DiscoverRow';

type Navigation = RootScreenProps<'OnboardDiscover'>['navigation'];

function Intro({
  title = onboardDiscover.findingTitle,
  body = native.onboard.body,
}: {
  title?: string;
  body?: string;
}) {
  return (
    <View style={styles.intro}>
      <AppText accessibilityRole="header" variant="title" weight={700}>
        {title}
      </AppText>
      <AppText color="text2" variant="body">
        {body}
      </AppText>
    </View>
  );
}

function LoadingDirectory() {
  return (
    <View accessible accessibilityLabel={onboardDiscover.a11y.loadingDirectory}>
      <View style={styles.workingRow}>
        <ActivityIndicator color={tokens.accent} size="small" />
        <AppText color="text2" variant="footnote">
          {onboardDiscover.loadingDirectory}
        </AppText>
      </View>
    </View>
  );
}

function Groups({
  groups,
  navigation,
}: {
  groups: DiscoverGroups;
  navigation: Navigation;
}) {
  const tailnetEnabled = useTailnetEnabled();
  // Every computer in the local directory is a paired Host, heartbeat or not.
  const remote = [...groups.remote, ...groups.noHost];
  const directCount = groups.nativeEntries.length + remote.length;

  return (
    <>
      {hasComputers(groups) ? null : (
        <Button
          fullWidth
          onPress={() => navigation.navigate('AddComputer')}
          title={native.home.add}
        />
      )}
      {directCount === 0 ? null : (
        <View style={styles.section}>
          <SectionHeader
            right={native.onboard.directCount(directCount)}
            title={native.onboard.direct}
          />
          <ListGroup>
            {groups.nativeEntries.map(entry => (
              <DiscoverRow
                entry={entry}
                key={entry.key}
                kind="native"
                onOpen={() =>
                  navigation.navigate('NativeSheet', { entryKey: entry.key })
                }
              />
            ))}
            {remote.map(computer => (
              <DiscoverRow
                computer={computer}
                key={computer.hostId}
                kind="remote"
                onPair={() =>
                  navigation.navigate('PairAdd', { hostId: computer.hostId })
                }
              />
            ))}
          </ListGroup>
        </View>
      )}
      {groups.ssh.length === 0 ? null : (
        <View style={styles.section}>
          <SectionHeader
            right={onboardDiscover.groups.sshCount(groups.ssh.length)}
            title={onboardDiscover.groups.ssh}
          />
          <ListGroup>
            {groups.ssh.map(server => (
              <DiscoverRow
                key={server.node}
                kind="ssh"
                onConfigure={() =>
                  navigation.navigate('AddServer', {
                    prefill: { name: server.node, host: server.addr },
                  })
                }
                server={server}
              />
            ))}
          </ListGroup>
        </View>
      )}
      <ListGroup>
        <ListRow
          chevron
          onPress={() => navigation.navigate('AddByAddress')}
          subtitle={
            tailnetEnabled
              ? onboardDiscover.addByAddress.subtitle
              : onboardDiscover.addByAddress.subtitleLan
          }
          title={onboardDiscover.addByAddress.title}
        />
      </ListGroup>
      <Button
        align="start"
        onPress={() => navigation.navigate('PairAdd')}
        title={native.home.otherWays}
        variant="text"
      />
    </>
  );
}

function DirectoryContent({ navigation }: { navigation: Navigation }) {
  const { directory, isLoading, error, refetch } = useDirectory();
  const { computers: manual } = useManualServers();
  const groups = useMemo(
    () =>
      directory === undefined ? null : groupForDiscover(directory, manual),
    [directory, manual],
  );
  const title = groups === null ? null : discoverTitle(groups);
  const previousDirectory = useRef(directory);

  useEffect(() => {
    const appeared =
      previousDirectory.current === undefined && directory !== undefined;
    previousDirectory.current = directory;
    if (appeared && title !== null) {
      // 标题会在目录出现时重新挂载，live region 不可靠；这里也没有别的 live
      // region，因此两端都主动播报一次，不会念两遍。
      AccessibilityInfo.announceForAccessibility(title);
    }
  }, [directory, title]);

  if (groups === null || title === null) {
    return (
      <>
        <Intro />
        {error !== null && !isLoading ? null : <LoadingDirectory />}
        {error !== null && !isLoading ? (
          <Banner
            action={{
              label: common.retry,
              onPress: () => {
                refetch().catch(() => undefined);
              },
            }}
            message={onboardDiscover.directoryFailed.message}
            title={onboardDiscover.directoryFailed.title}
            tone="bad"
          />
        ) : null}
      </>
    );
  }

  return (
    <>
      <Intro
        body={hasComputers(groups) ? undefined : native.home.emptyBody}
        title={title}
      />
      <Groups groups={groups} navigation={navigation} />
    </>
  );
}

export default function OnboardDiscoverScreen({
  navigation,
}: RootScreenProps<'OnboardDiscover'>) {
  const largeText = PixelRatio.getFontScale() > 2;

  const footer = (
    <View
      style={[styles.footer, largeText ? styles.footerLarge : null]}
      testID="onboard-discover-footer"
    >
      <View
        style={largeText ? styles.continueButtonLarge : styles.continueButton}
      >
        <Button
          fullWidth
          onPress={() => navigation.navigate('OnboardProtect')}
          title={common.continue}
          variant="primaryLarge"
        />
      </View>
    </View>
  );

  return (
    <SafeAreaView edges={['top', 'bottom']} style={styles.screen}>
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
        testID="onboard-discover-scroll"
      >
        <StepHeader step="discover" />
        <DirectoryContent navigation={navigation} />
        {largeText ? footer : null}
      </ScrollView>
      {largeText ? null : footer}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
    paddingHorizontal: space.gutter,
  },
  content: {
    gap: space.section,
    paddingTop: space.sm,
    paddingBottom: space.section,
  },
  intro: {
    gap: space.md,
  },
  workingRow: {
    minHeight: 32,
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
  },
  section: {
    gap: space.sm,
  },
  footer: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
    paddingTop: space.md,
    paddingBottom: space.sm,
  },
  continueButton: {
    flex: 1,
  },
  footerLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  continueButtonLarge: {
    alignSelf: 'stretch',
  },
});
