import { useRef } from 'react';
import type { JSX } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { useCountdown } from '../../components/Countdown';
import { Dialog } from '../../components/Dialog';
import { adapters } from '../../i18n/zh/adapters';
import { space } from '../../theme/tokens';
import { fingerprintLines } from './logic';

/** The adapter rejects an unanswered prompt after this long (V4 C1 §56). */
export const CERT_PROMPT_SECONDS = 60;

export type CertPromptProps = {
  fp: string;
  oldFp?: string;
  onAnswer: (accept: boolean) => void;
};

function Fingerprint({
  label,
  fp,
  testID,
}: {
  label: string;
  fp: string;
  testID: string;
}): JSX.Element {
  return (
    <View
      accessible
      accessibilityLabel={`${label} ${fp}`}
      style={styles.fingerprint}
      testID={testID}
    >
      <AppText color="text2" variant="caption">
        {label}
      </AppText>
      {fingerprintLines(fp).map((line, index) => (
        <AppText key={index} mono selectable variant="mono">
          {line}
        </AppText>
      ))}
    </View>
  );
}

export function CertPrompt(p: CertPromptProps): JSX.Element {
  const answeredRef = useRef(false);
  const remaining = useCountdown(CERT_PROMPT_SECONDS, { running: true });

  const answer = (accept: boolean) => {
    if (answeredRef.current) {
      return;
    }
    answeredRef.current = true;
    p.onAnswer(accept);
  };

  return (
    <Dialog
      actions={[
        {
          label: adapters.cert.reject,
          onPress: () => answer(false),
          variant: 'secondary',
        },
        {
          label: adapters.cert.trust,
          onPress: () => answer(true),
          variant: 'pillDanger',
        },
      ]}
      message={adapters.cert.message}
      onDismiss={() => answer(false)}
      testID="adapter-cert"
      title={adapters.cert.title}
      visible
    >
      <View style={styles.fingerprints}>
        {p.oldFp === undefined ? null : (
          <Fingerprint
            fp={p.oldFp}
            label={adapters.cert.oldFp}
            testID="adapter-cert-old"
          />
        )}
        <Fingerprint
          fp={p.fp}
          label={adapters.cert.newFp}
          testID="adapter-cert-new"
        />
        <AppText
          accessibilityLiveRegion={remaining <= 10 ? 'polite' : 'none'}
          color="text3"
          testID="adapter-cert-countdown"
          variant="caption"
        >
          {adapters.cert.countdown(remaining)}
        </AppText>
      </View>
    </Dialog>
  );
}

const styles = StyleSheet.create({
  fingerprints: {
    gap: space.sm,
  },
  fingerprint: {
    gap: 2,
  },
});

export default CertPrompt;
