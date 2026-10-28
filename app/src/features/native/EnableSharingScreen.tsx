import { useEffect, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { IconButton } from '../../components/IconButton';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { radius, space, tokens } from '../../theme/tokens';
import { FallbackGuide } from '../fallback/FallbackGuide';
import { useComputerEntries } from './useComputerEntries';
import { useConnect } from './useConnect';

/** V10 C8: how to turn on Screen Sharing / Remote Desktop, then retry C7. */
export default function EnableSharingScreen({
  navigation,
  route,
}: RootScreenProps<'EnableSharing'>) {
  const { os, name, entryKey } = route.params;
  const insets = useSafeAreaInsets();
  const entries = useComputerEntries();
  const { connect, busy, guide, closeGuide } = useConnect();
  const [stillOff, setStillOff] = useState(false);
  const steps =
    os === 'macos' ? native.enable.stepsMac : native.enable.stepsWin;

  useEffect(() => {
    if (stillOff && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(native.enable.stillOff(os));
    }
  }, [os, stillOff]);

  const retry = () => {
    const entry =
      entryKey === undefined
        ? undefined
        : entries.find(candidate => candidate.key === entryKey);
    if (entry === undefined) {
      // The list changed, or the caller had no entry: go back to it.
      navigation.goBack();
      return;
    }
    setStillOff(false);
    // Probe from this page: still off stays here and says so; a session
    // replaces the guide, so leaving it does not land back on these steps.
    connect(entry, {
      replace: true,
      onSharingOff: () => setStillOff(true),
    }).catch(() => undefined);
  };

  return (
    <View style={styles.screen}>
      <ScrollView
        contentContainerStyle={[
          styles.content,
          {
            paddingTop: insets.top + 8,
            paddingBottom: insets.bottom + 24,
          },
        ]}
        showsVerticalScrollIndicator={false}
      >
        <View style={styles.header}>
          <IconButton
            accessibilityLabel={common.a11y.back}
            icon="chevronLeft"
            onPress={() => navigation.goBack()}
            size={44}
            variant="raised"
          />
        </View>
        <AppText accessibilityRole="header" variant="title" weight={700}>
          {native.enable.title(os)}
        </AppText>
        <AppText color="text2" variant="body">
          {native.enable.lead(os, name)}
        </AppText>
        <View style={styles.steps} testID="enable-sharing-steps">
          {steps.map((step, index) => (
            <View accessible key={step} style={styles.step}>
              <View style={styles.number}>
                <AppText color="accent" variant="footnote" weight={700}>
                  {String(index + 1)}
                </AppText>
              </View>
              <AppText style={styles.stepText} variant="body">
                {step}
              </AppText>
            </View>
          ))}
        </View>
        {stillOff ? (
          <AppText
            accessibilityLiveRegion="polite"
            color="bad"
            testID="enable-sharing-still-off"
            variant="footnote"
          >
            {native.enable.stillOff(os)}
          </AppText>
        ) : null}
        <Button
          disabled={busy}
          fullWidth
          loading={busy}
          onPress={retry}
          testID="enable-sharing-retry"
          title={native.enable.retry}
          variant="primaryLarge"
        />
        <Button
          align="start"
          onPress={() =>
            navigation.navigate('PairAdd', undefined, { pop: true })
          }
          title={native.enable.installHost}
          variant="text"
        />
      </ScrollView>
      <FallbackGuide guide={guide} onClose={closeGuide} />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    paddingHorizontal: space.gutter,
    gap: space.md,
  },
  header: {
    minHeight: 44,
    justifyContent: 'center',
  },
  steps: {
    gap: space.md,
    paddingVertical: space.sm,
  },
  step: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: space.md,
  },
  number: {
    width: 24,
    height: 24,
    borderRadius: radius.pill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
  stepText: {
    flex: 1,
  },
});
