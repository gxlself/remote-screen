import { useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, Platform, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { StatusChip } from '../../components/StatusChip';
import { common } from '../../i18n/zh/common';
import { sshKeys } from '../../i18n/zh/sshKeys';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { SshKey } from '../../native/rscore/types';
import { keyCreatedOn, keyKindLabel } from '../../features/ssh/keysView';
import { radius, space, tokens } from '../../theme/tokens';

const COPIED_RESET_MS = 2000;

export type KeyCardProps = { item: SshKey; onCopyFailed?: () => void };

export function KeyCard({ item: key, onCopyFailed }: KeyCardProps) {
  const platform = usePlatform();
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
      }
    },
    [],
  );

  const copy = async () => {
    try {
      await platform.copyText(key.publicKey);
    } catch {
      onCopyFailed?.();
      return;
    }
    setCopied(true);
    if (Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(common.copied);
    }
    if (timer.current !== null) {
      clearTimeout(timer.current);
    }
    timer.current = setTimeout(() => {
      timer.current = null;
      setCopied(false);
    }, COPIED_RESET_MS);
  };

  const share = () => {
    platform
      .share({ text: key.publicKey, title: key.name })
      .catch(() => undefined);
  };

  const description =
    key.kind === 'se-p256'
      ? sshKeys.keyCard.seDescription
      : sshKeys.keyCard.edDescription;

  return (
    <View
      accessibilityLabel={sshKeys.keyCard.a11y}
      style={styles.card}
      testID="key-card"
    >
      <View style={styles.heading}>
        <AppText style={styles.name} variant="headline" weight={600}>
          {key.name}
        </AppText>
        <StatusChip
          label={sshKeys.keyCard.installedCount(key.installedOn.length)}
          tone={key.installedOn.length === 0 ? 'neutral' : 'good'}
        />
      </View>

      <AppText color="text2" variant="footnote">
        {keyKindLabel(key.kind)} ·{' '}
        {sshKeys.keyCard.generatedOn(keyCreatedOn(key.createdAt))}
      </AppText>

      <AppText color="text3" variant="footnote">
        {description}
      </AppText>

      <View
        accessible
        accessibilityLabel={sshKeys.keyCard.publicKeyA11y}
        accessibilityHint={sshKeys.keyCard.publicKeyHint}
        style={styles.publicKey}
        testID="public-key"
      >
        <AppText
          ellipsizeMode="middle"
          mono
          numberOfLines={3}
          style={styles.publicKeyText}
          variant="monoSmall"
        >
          {key.publicKey}
        </AppText>
        <AppText color="text3" mono style={styles.fp} variant="tiny">
          {key.fp}
        </AppText>
      </View>

      <View style={styles.actions} testID="key-card-actions">
        <View accessibilityLiveRegion="polite" testID="key-card-copy-live">
          <Button
            onPress={copy}
            title={copied ? common.copied : sshKeys.keyCard.copy}
            variant="secondary"
          />
        </View>
        <Button
          icon="share"
          onPress={share}
          title={sshKeys.keyCard.share}
          variant="secondary"
        />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    gap: space.md,
    padding: space.lg,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  heading: {
    minHeight: 26,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: space.sm,
  },
  name: { flexShrink: 1 },
  publicKey: {
    gap: space.xs,
    padding: space.md,
    borderRadius: radius.button,
    backgroundColor: tokens.raised,
  },
  publicKeyText: { color: tokens.text },
  fp: { letterSpacing: 0 },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: space.sm,
  },
});

export default KeyCard;
