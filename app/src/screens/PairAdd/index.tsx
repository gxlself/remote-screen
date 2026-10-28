import { useEffect, useRef, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { FormScroll } from '../../components/FormScroll';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { TextField } from '../../components/TextField';
import { Toast, useToast } from '../../components/Toast';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { Computer } from '../../native/rscore/types';
import { useDirectory } from '../../features/data/queries';
import { isCompleteCode } from '../../features/pairing/codeInput';
import { common } from '../../i18n/zh/common';
import { pairAdd } from '../../i18n/zh/pairAdd';
import { tokens } from '../../theme/tokens';
import { formatPairAddress, parsePairAddress } from './address';
import CodeBoxes from './CodeBoxes';
import type { CodeBoxesHandle } from './CodeBoxes';
import ScannerSlot from './ScannerSlot';

type PairTab = 'scan' | 'code';

const DUPLICATE_SCAN_WINDOW_MS = 2000;

/** The address to pair a named computer at: its Tailscale one first. */
function computerAddress(computer: Computer | undefined): string | undefined {
  return (
    computer?.addresses.find(address => address.kind === 'tailnet') ??
    computer?.addresses[0]
  )?.addr;
}

function useScreenFocused(
  navigation: RootScreenProps<'PairAdd'>['navigation'],
): boolean {
  const [focused, setFocused] = useState(() => navigation.isFocused());
  useEffect(() => {
    const offFocus = navigation.addListener('focus', () => setFocused(true));
    const offBlur = navigation.addListener('blur', () => setFocused(false));
    return () => {
      offFocus();
      offBlur();
    };
  }, [navigation]);
  return focused;
}

export default function PairAddScreen({
  navigation,
  route,
}: RootScreenProps<'PairAdd'>) {
  const core = useCore();
  const platform = usePlatform();
  const insets = useSafeAreaInsets();
  const { toast, show: showToast, hide: hideToast } = useToast();
  const { directory } = useDirectory();
  const focused = useScreenFocused(navigation);
  const params = route.params;
  const [tab, setTab] = useState<PairTab>(
    params?.tab ??
      (params?.hostId === undefined && params?.addr === undefined
        ? 'scan'
        : 'code'),
  );
  const [code, setCode] = useState('');
  const [addrText, setAddrText] = useState(() =>
    formatPairAddress(params?.addr, params?.port),
  );
  const addrEdited = useRef(false);
  const lastScan = useRef<{ text: string; at: number } | null>(null);
  const rejectedScans = useRef(new Set<string>());
  const submittedCode = useRef<string | null>(null);
  const codeRef = useRef<CodeBoxesHandle>(null);

  // Opened for a named computer (a failed scan, onboarding's 配对): fill in
  // where it lives once the directory knows it.
  const knownAddr =
    params?.addr === undefined && params?.hostId !== undefined
      ? computerAddress(
          directory?.computers.find(item => item.hostId === params.hostId),
        )
      : undefined;
  useEffect(() => {
    if (knownAddr !== undefined && !addrEdited.current) {
      setAddrText(current => (current === '' ? knownAddr : current));
    }
  }, [knownAddr]);

  const address = parsePairAddress(addrText);

  const onScan = (text: string) => {
    const now = Date.now();
    const previous = lastScan.current;
    if (
      previous !== null &&
      previous.text === text &&
      now - previous.at < DUPLICATE_SCAN_WINDOW_MS
    ) {
      return;
    }
    lastScan.current = { text, at: now };
    if (core.pairing.parseQr(text) === null) {
      // The camera keeps seeing the same wrong code; say so once.
      if (!rejectedScans.current.has(text)) {
        rejectedScans.current.add(text);
        platform.haptic('error');
        showToast(pairAdd.scan.invalid, { tone: 'bad' });
      }
      return;
    }
    platform.haptic('success');
    navigation.replace('PairWaiting', { qrText: text });
  };

  const canSubmitAddress = address !== null && isCompleteCode(code);
  const submitAddress = (nextCode: string) => {
    if (address === null || !isCompleteCode(nextCode)) {
      return;
    }
    const key = `addr:${address.addr}:${address.port ?? ''}:${nextCode}`;
    if (submittedCode.current === key) {
      return;
    }
    submittedCode.current = key;
    navigation.replace('PairWaiting', { ...address, code: nextCode });
  };

  const onAddressCodeChange = (nextCode: string) => {
    setCode(nextCode);
    submitAddress(nextCode);
  };

  return (
    <FormScroll
      contentContainerStyle={[
        styles.content,
        { paddingBottom: insets.bottom + 24 },
      ]}
      footer={<Toast onHide={hideToast} toast={toast} />}
      header={
        <View style={styles.header}>
          <IconButton
            accessibilityLabel={common.a11y.close}
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
            {pairAdd.title}
          </AppText>
          <View style={styles.headerSpacer} />
        </View>
      }
      style={[styles.screen, { paddingTop: insets.top + 8 }]}
      testID="pair-add-screen"
    >
      <Segmented
        accessibilityLabel={pairAdd.methodLabel}
        onChange={setTab}
        options={[
          { value: 'scan', label: pairAdd.scan.tab },
          { value: 'code', label: pairAdd.code.tab },
        ]}
        testID="pair-tabs"
        value={tab}
        variant="page"
      />
      {tab === 'scan' ? (
        <View style={styles.section} testID="pair-scan-tab">
          <ScannerSlot
            active={tab === 'scan' && focused}
            onScan={onScan}
            onUseCode={() => setTab('code')}
          />
          <AppText color="text2" variant="footnote">
            {pairAdd.scan.instructions}
          </AppText>
        </View>
      ) : (
        <View style={styles.section} testID="pair-code-tab">
          <TextField
            autoComplete="off"
            blurOnSubmit={false}
            keyboardType="url"
            label={pairAdd.byAddress.addrLabel}
            mono
            onChangeText={value => {
              addrEdited.current = true;
              setAddrText(value);
            }}
            onSubmitEditing={() => codeRef.current?.focus()}
            placeholder={pairAdd.byAddress.addrPlaceholder}
            returnKeyType="next"
            testID="pair-addr-input"
            textContentType="URL"
            value={addrText}
          />
          <View style={styles.codeField}>
            <AppText color="text2" variant="footnote" weight={500}>
              {pairAdd.byAddress.codeLabel}
            </AppText>
            <CodeBoxes
              ref={codeRef}
              // With the address already there, the code is all that is left.
              autoFocus={params?.addr !== undefined}
              onChange={onAddressCodeChange}
              value={code}
            />
            <AppText color="text2" variant="footnote">
              {pairAdd.byAddress.codeHint}
            </AppText>
          </View>
          <Button
            disabled={!canSubmitAddress}
            fullWidth
            onPress={() => submitAddress(code)}
            testID="pair-submit"
            title={pairAdd.byAddress.start}
            variant="primaryLarge"
          />
        </View>
      )}
      <View style={styles.footer}>
        <View style={styles.section}>
          <SectionHeader title={pairAdd.other.title} />
          <ListGroup>
            <ListRow
              chevron
              onPress={() => navigation.navigate('PairFile')}
              title={pairAdd.other.importPairFile}
            />
          </ListGroup>
        </View>
        {/* Back to the page when it is already open under this modal; the
            modal is replaced otherwise, so nothing piles up. */}
        <ListGroup>
          <ListRow
            chevron
            onPress={() => navigation.popTo('AddByAddress')}
            title={pairAdd.links.byAddress}
          />
          <ListRow
            chevron
            onPress={() => navigation.popTo('AddServer')}
            title={pairAdd.links.sshServer}
          />
        </ListGroup>
      </View>
    </FormScroll>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  header: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
  },
  headerTitle: {
    flex: 1,
    textAlign: 'center',
  },
  headerSpacer: {
    width: 44,
  },
  content: {
    paddingTop: 12,
    paddingHorizontal: 20,
    gap: 24,
  },
  section: {
    gap: 16,
  },
  codeField: {
    gap: 10,
  },
  footer: {
    gap: 12,
  },
});
