import { useEffect, useRef, useState } from 'react';
import { PixelRatio, StyleSheet, View } from 'react-native';
import type { SshAuth, SshKey } from '../../native/rscore/types';
import { Button } from '../../components/Button';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { addServer } from '../../i18n/zh/addServer';
import { common } from '../../i18n/zh/common';
import { radius, space, tokens } from '../../theme/tokens';

type AuthSectionProps = {
  auth: SshAuth;
  keys: SshKey[];
  tailscaleUnavailable: boolean;
  readOnly: boolean;
  /** Validation message shown once the user tried to save or test. */
  error?: string;
  onOpenKeys: () => void;
  onCopy: (text: string) => Promise<void>;
};

export function AuthSection({
  auth,
  keys,
  tailscaleUnavailable,
  readOnly,
  onOpenKeys,
  onCopy,
  error,
}: AuthSectionProps) {
  const largeText = PixelRatio.getFontScale() > 2;
  const errorLine =
    error === undefined ? null : (
      <AppText color="bad" variant="footnote" testID="auth-error">
        {error}
      </AppText>
    );

  if (auth === 'password') {
    return (
      <AppText color="text2" variant="footnote" testID="auth-password-note">
        {addServer.auth.password.note}
      </AppText>
    );
  }

  if (auth === 'tailscale') {
    // The note already says why; after a failed save it turns red instead of
    // repeating the same sentence underneath.
    return (
      <AppText
        color={
          error !== undefined ? 'bad' : tailscaleUnavailable ? 'relay' : 'text2'
        }
        variant="footnote"
        testID="auth-tailscale-note"
      >
        {tailscaleUnavailable
          ? addServer.auth.tailscale.unavailable
          : addServer.auth.tailscale.ready}
      </AppText>
    );
  }

  if (keys.length === 0) {
    return (
      <>
        <View
          style={[styles.card, largeText ? styles.cardLarge : null]}
          testID="auth-key-empty"
        >
          <AppText color="text2" variant="body">
            {addServer.auth.key.empty}
          </AppText>
          <Button
            disabled={readOnly}
            fullWidth={largeText}
            onPress={onOpenKeys}
            title={addServer.auth.key.create}
            variant="secondary"
          />
        </View>
        {errorLine}
      </>
    );
  }

  return (
    <>
      <KeyCard keyInfo={keys[0]} onCopy={onCopy} largeText={largeText} />
      {errorLine}
    </>
  );
}

function KeyCard({
  keyInfo,
  onCopy,
  largeText,
}: {
  keyInfo: SshKey;
  onCopy: (text: string) => Promise<void>;
  largeText: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
        timer.current = null;
      }
    },
    [],
  );

  const copy = async () => {
    try {
      await onCopy(keyInfo.publicKey);
    } catch {
      setCopied(false);
      setCopyFailed(true);
      return;
    }
    setCopyFailed(false);
    setCopied(true);
    if (timer.current !== null) {
      clearTimeout(timer.current);
    }
    timer.current = setTimeout(() => {
      timer.current = null;
      setCopied(false);
    }, 2000);
  };

  const keyNote =
    keyInfo.kind === 'ed25519'
      ? addServer.auth.key.ed25519
      : addServer.auth.key.move;

  return (
    <View
      style={[styles.card, largeText ? styles.cardLarge : null]}
      testID="auth-key-card"
    >
      <Icon color="text2" name="key" size={20} />
      <View style={styles.keyText}>
        <AppText variant="bodyStrong">{addServer.auth.key.use}</AppText>
        <AppText color="text2" variant="monoSmall">
          {keyInfo.name}
        </AppText>
        <AppText color="text3" variant="caption">
          {keyNote}
        </AppText>
        {copyFailed ? (
          <AppText color="bad" variant="caption" testID="auth-key-copy-failed">
            {addServer.auth.key.copyFailed}
          </AppText>
        ) : null}
      </View>
      <Button
        fullWidth={largeText}
        onPress={() => {
          copy();
        }}
        title={copied ? common.copied : addServer.auth.key.copy}
        variant="secondary"
      />
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.md,
    padding: space.md,
    borderRadius: radius.button,
    backgroundColor: tokens.card,
  },
  cardLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  keyText: {
    flex: 1,
    gap: 2,
  },
});

export default AuthSection;
