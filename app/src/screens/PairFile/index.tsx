import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { Button } from '../../components/Button';
import { FormScroll } from '../../components/FormScroll';
import { IconButton } from '../../components/IconButton';
import { TextField } from '../../components/TextField';
import { fpDisplay } from '../../features/format/fingerprint';
import { formatHm } from '../../features/format/time';
import { rememberPairFile } from '../../features/pairing/pairFileCache';
import { common } from '../../i18n/zh/common';
import { pairFile } from '../../i18n/zh/pairFile';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { PairFileSummary } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';

type PairFileSource = { text: string } | { uri: string };

type LoadError = {
  message: string;
  retry: boolean;
};

function loadError(error: unknown): LoadError {
  if (error instanceof RsCoreError) {
    if (error.kind === 'internal' && error.message === 'not a pairing file') {
      return { message: pairFile.errors.notPairFile, retry: false };
    }
    if (error.kind === 'unsupported') {
      return { message: pairFile.errors.unsupported, retry: false };
    }
  }
  return { message: pairFile.errors.generic, retry: true };
}

function expiryText(expiresAt: number): string {
  const date = new Date(Math.floor(expiresAt / 1000));
  const monthDay = common.time.monthDay(date.getMonth() + 1, date.getDate());
  return pairFile.summary.validUntil(`${monthDay} ${formatHm(expiresAt)}`);
}

function shortFingerprint(fp: string): string {
  return fpDisplay(fp).split(' ').slice(0, 2).join(' ');
}

function summaryAccessibilityLabel(summary: PairFileSummary): string {
  return pairFile.a11y.summaryText({
    hostName: summary.hostName,
    expired: summary.expired,
    fp: fpDisplay(summary.fp),
    addr: summary.addrs[0],
    validUntil: expiryText(summary.expiresAt),
  });
}

function SummaryCard({ summary }: { summary: PairFileSummary }) {
  const accessibilityLabel = summaryAccessibilityLabel(summary);
  const largeText = PixelRatio.getFontScale() > 2;
  return (
    <View
      accessible
      accessibilityLabel={accessibilityLabel}
      accessibilityLiveRegion="polite"
      style={styles.summaryCard}
      testID="pair-file-summary"
    >
      <View style={styles.summaryHeading}>
        <View style={styles.summaryCopy}>
          <AppText color="text3" variant="caption">
            {pairFile.summary.computer}
          </AppText>
          <AppText variant="headline" weight={600}>
            {summary.hostName}
          </AppText>
        </View>
        {summary.expired ? (
          <View style={styles.expiredBadge} testID="pair-file-expired">
            <AppText color="bad" variant="caption" weight={600}>
              {pairFile.summary.expired}
            </AppText>
          </View>
        ) : null}
      </View>
      <View style={styles.summaryRows}>
        <View
          style={[styles.summaryRow, largeText ? styles.summaryRowLarge : null]}
        >
          <AppText color="text3" variant="caption" style={styles.summaryLabel}>
            {pairFile.summary.fingerprint}
          </AppText>
          <AppText mono variant="mono" testID="pair-file-fingerprint">
            {shortFingerprint(summary.fp)}
          </AppText>
        </View>
        {summary.addrs[0] === undefined ? null : (
          <View
            style={[
              styles.summaryRow,
              largeText ? styles.summaryRowLarge : null,
            ]}
          >
            <AppText
              color="text3"
              variant="caption"
              style={styles.summaryLabel}
            >
              {pairFile.summary.address}
            </AppText>
            <AppText mono variant="mono" testID="pair-file-address">
              {summary.addrs[0]}
            </AppText>
          </View>
        )}
        <AppText color="text2" variant="footnote" testID="pair-file-expiry">
          {expiryText(summary.expiresAt)}
        </AppText>
      </View>
      {summary.expired ? (
        <AppText color="text2" variant="footnote">
          {pairFile.summary.expiredHint}
        </AppText>
      ) : null}
    </View>
  );
}

export default function PairFileScreen({
  navigation,
  route,
}: RootScreenProps<'PairFile'>) {
  const core = useCore();
  const platform = usePlatform();
  const insets = useSafeAreaInsets();
  const initialText = route.params?.text ?? '';
  const [text, setText] = useState(initialText);
  const [summary, setSummary] = useState<PairFileSummary | null>(null);
  const [error, setError] = useState<LoadError | null>(null);
  const [clipboardNotice, setClipboardNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const mountedRef = useRef(false);
  const attemptRef = useRef(0);
  const loadedParamRef = useRef<string | null>(null);
  const lastSourceRef = useRef<PairFileSource | null>(null);
  const previousSummaryRef = useRef<PairFileSummary | null>(null);

  useEffect(() => {
    if (
      summary !== null &&
      previousSummaryRef.current === null &&
      Platform.OS === 'ios'
    ) {
      AccessibilityInfo.announceForAccessibility(
        summaryAccessibilityLabel(summary),
      );
    }
    previousSummaryRef.current = summary;
  }, [summary]);

  const load = useCallback(
    async (source: PairFileSource, clearParams = false) => {
      const attempt = attemptRef.current + 1;
      attemptRef.current = attempt;
      lastSourceRef.current = source;
      setLoading(true);
      setError(null);
      setClipboardNotice(null);
      setSummary(null);
      try {
        const next = await core.pairing.loadFile(source);
        if (!mountedRef.current || attempt !== attemptRef.current) {
          return;
        }
        rememberPairFile(next);
        setSummary(next);
        setText('');
        lastSourceRef.current = null;
        if (clearParams) {
          navigation.setParams({ text: undefined, uri: undefined });
        }
      } catch (cause) {
        if (!mountedRef.current || attempt !== attemptRef.current) {
          return;
        }
        setSummary(null);
        setError(loadError(cause));
      } finally {
        if (mountedRef.current && attempt === attemptRef.current) {
          setLoading(false);
        }
      }
    },
    [core, navigation],
  );

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      attemptRef.current += 1;
      lastSourceRef.current = null;
    };
  }, []);

  useEffect(() => {
    const textParam = route.params?.text;
    const uriParam = route.params?.uri;
    const source: PairFileSource | null =
      textParam !== undefined
        ? { text: textParam }
        : uriParam !== undefined
        ? { uri: uriParam }
        : null;
    if (source === null) {
      return;
    }
    const key = 'text' in source ? `text:${source.text}` : `uri:${source.uri}`;
    if (loadedParamRef.current === key) {
      return;
    }
    loadedParamRef.current = key;
    if ('text' in source) {
      setText(source.text);
    }
    load(source, true);
  }, [load, route.params?.text, route.params?.uri]);

  const readFromInput = () => {
    const value = text.trim();
    if (value === '') {
      return;
    }
    load({ text: value });
  };

  const paste = async () => {
    setClipboardNotice(null);
    try {
      const value = await platform.readText();
      if (!mountedRef.current) {
        return;
      }
      if (value === null) {
        setClipboardNotice(pairFile.clipboardEmpty);
        return;
      }
      setText(value);
      setError(null);
    } catch {
      if (mountedRef.current) {
        setClipboardNotice(pairFile.clipboardError);
      }
    }
  };

  const retry = () => {
    const source = lastSourceRef.current;
    if (source !== null) {
      load(source);
      return;
    }
    readFromInput();
  };

  const pair = () => {
    if (summary === null || summary.expired) {
      return;
    }
    navigation.replace('PairWaiting', { fileId: summary.fileId });
  };

  return (
    <FormScroll
      contentContainerStyle={[
        styles.content,
        { paddingBottom: insets.bottom + 24 },
      ]}
      header={
        <View style={styles.header}>
          <IconButton
            accessibilityLabel={pairFile.a11y.close}
            icon="close"
            onPress={() => navigation.goBack()}
            size={44}
            variant="raised"
          />
          <AppText
            accessibilityRole="header"
            variant="headline"
            weight={600}
            style={styles.headerTitle}
          >
            {pairFile.title}
          </AppText>
          <View style={styles.headerSpacer} />
        </View>
      }
      style={[styles.screen, { paddingTop: insets.top + 8 }]}
      testID="pair-file-screen"
    >
      <AppText color="text2" variant="body">
        {Platform.OS === 'android' ? pairFile.introAndroid : pairFile.intro}
      </AppText>
      <View style={styles.form}>
        <TextField
          autoCapitalize="none"
          autoCorrect={false}
          accessibilityLabel={pairFile.input.label}
          label={pairFile.input.label}
          mono
          onChangeText={value => {
            setText(value);
            setError(null);
            setClipboardNotice(null);
          }}
          placeholder={pairFile.input.placeholder}
          testID="pair-file-input"
          value={text}
        />
        <View
          style={[
            styles.inputActions,
            PixelRatio.getFontScale() > 2 ? styles.inputActionsLarge : null,
          ]}
        >
          <Button
            onPress={() => {
              paste();
            }}
            testID="pair-file-paste"
            title={pairFile.buttons.paste}
            variant="secondary"
          />
          <Button
            disabled={text.trim() === ''}
            loading={loading}
            onPress={readFromInput}
            testID="pair-file-read"
            title={pairFile.buttons.read}
          />
        </View>
      </View>

      {clipboardNotice === null ? null : (
        <Banner
          icon="clipboard"
          testID="pair-file-clipboard-notice"
          title={clipboardNotice}
          tone="warn"
        />
      )}
      {error === null ? null : (
        <Banner
          action={
            error.retry
              ? { label: pairFile.errors.retry, onPress: retry }
              : undefined
          }
          icon="warning"
          testID="pair-file-error"
          title={error.message}
          tone="bad"
        />
      )}
      {summary === null ? null : (
        <>
          <SummaryCard summary={summary} />
          <Banner
            icon="shield"
            testID="pair-file-warning"
            title={pairFile.warning}
            tone="warn"
          />
          <Button
            disabled={summary.expired}
            fullWidth
            onPress={pair}
            testID="pair-file-pair"
            title={pairFile.buttons.pair}
            variant="primaryLarge"
          />
        </>
      )}
    </FormScroll>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: tokens.bg },
  header: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
  },
  headerTitle: { flex: 1, textAlign: 'center' },
  headerSpacer: { width: 44 },
  content: { paddingTop: 12, paddingHorizontal: 20, gap: 20 },
  form: { gap: 12 },
  inputActions: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    gap: 12,
  },
  inputActionsLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  summaryCard: {
    gap: 14,
    padding: 16,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  summaryHeading: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    justifyContent: 'space-between',
    gap: 12,
  },
  summaryCopy: { flex: 1, gap: 2 },
  summaryRows: { gap: 8 },
  summaryRow: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  summaryRowLarge: { flexWrap: 'wrap' },
  summaryLabel: { minWidth: 36 },
  expiredBadge: {
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 8,
    backgroundColor: tokens.badSoft,
  },
});
