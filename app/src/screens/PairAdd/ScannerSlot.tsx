import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Animated,
  AppState,
  Easing,
  Linking,
  Platform,
  Pressable,
  StyleSheet,
  UIManager,
  View,
} from 'react-native';
import type { NativeSyntheticEvent, ViewProps } from 'react-native';
import Svg, { Defs, RadialGradient, Rect, Stop } from 'react-native-svg';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { TextField } from '../../components/TextField';
import { pairAdd } from '../../i18n/zh/pairAdd';
import RsQrScannerView from '../../native/specs/RsQrScannerNativeComponent';
import { tokens } from '../../theme/tokens';

export type ScannerSlotProps = {
  onScan: (text: string) => void;
  active: boolean;
  /** No camera: switch to typing the code. */
  onUseCode?: () => void;
  testID?: string;
};

type ScannerSlotHostProps = ViewProps & Pick<ScannerSlotProps, 'onScan'>;
type ScannerCodeEvent = NativeSyntheticEvent<Readonly<{ text: string }>>;
type ScannerStateEvent = NativeSyntheticEvent<
  Readonly<{ state: string; message: string; hasTorch: boolean }>
>;

const VIEWFINDER_HEIGHT = 246;
const VIEWFINDER_MAX_WIDTH = 420;
const SCAN_LINE_TOP = 26;
const SCAN_LINE_BOTTOM = 46;
const SCAN_LINE_HEIGHT = 2;
const SCAN_LINE_TRAVEL =
  VIEWFINDER_HEIGHT - SCAN_LINE_TOP - SCAN_LINE_BOTTOM - SCAN_LINE_HEIGHT;
const VIEWFINDER_GRADIENT_START = '#1A2329';
const VIEWFINDER_GRADIENT_END = '#07090B';

function ViewfinderGradient() {
  return (
    <Svg
      accessibilityElementsHidden
      accessible={false}
      height="100%"
      importantForAccessibility="no-hide-descendants"
      pointerEvents="none"
      style={StyleSheet.absoluteFill}
      width="100%"
    >
      <Defs>
        <RadialGradient
          cx="50%"
          cy="42%"
          gradientUnits="objectBoundingBox"
          id="rs-scanner-viewfinder-gradient"
          r="78%"
        >
          <Stop offset="0%" stopColor={VIEWFINDER_GRADIENT_START} />
          <Stop offset="100%" stopColor={VIEWFINDER_GRADIENT_END} />
        </RadialGradient>
      </Defs>
      <Rect
        fill="url(#rs-scanner-viewfinder-gradient)"
        height="100%"
        width="100%"
      />
    </Svg>
  );
}

function ScannerLine({
  animate,
  testID,
}: {
  animate: boolean;
  testID: string;
}) {
  const progress = useRef(new Animated.Value(0.4)).current;

  useEffect(() => {
    if (!animate) {
      progress.setValue(0.4);
      return undefined;
    }

    progress.setValue(0);
    const animation = Animated.loop(
      Animated.sequence([
        Animated.timing(progress, {
          toValue: 1,
          duration: 1600,
          easing: Easing.inOut(Easing.ease),
          useNativeDriver: true,
        }),
        Animated.timing(progress, {
          toValue: 0,
          duration: 1600,
          easing: Easing.inOut(Easing.ease),
          useNativeDriver: true,
        }),
      ]),
    );
    animation.start();

    return () => {
      animation.stop();
    };
  }, [animate, progress]);

  const translateY = progress.interpolate({
    inputRange: [0, 1],
    outputRange: [0, SCAN_LINE_TRAVEL],
  });

  return (
    <Animated.View
      pointerEvents="none"
      style={[styles.scanLine, { transform: [{ translateY }] }]}
      testID={testID}
    />
  );
}

function ViewfinderGuide({
  animate,
  testID,
}: {
  animate: boolean;
  testID: string;
}) {
  return (
    <>
      <View pointerEvents="none" style={StyleSheet.absoluteFill}>
        <View style={[styles.corner, styles.cornerTopLeft]} />
        <View style={[styles.corner, styles.cornerTopRight]} />
        <View style={[styles.corner, styles.cornerBottomLeft]} />
        <View style={[styles.corner, styles.cornerBottomRight]} />
      </View>
      <ScannerLine animate={animate} testID={`${testID}-scan-line`} />
      <AppText
        center
        color="text2"
        pointerEvents="none"
        style={styles.runningHint}
        variant="footnote"
      >
        {pairAdd.scan.hint}
      </AppText>
    </>
  );
}

export function ScannerSlot({
  onScan,
  active,
  onUseCode,
  testID = 'scanner-slot',
}: ScannerSlotProps) {
  const [pastedText, setPastedText] = useState('');
  const [scannerState, setScannerState] = useState('starting');
  const [scannerKey, setScannerKey] = useState(0);
  const [hasTorch, setHasTorch] = useState(false);
  const [torchEnabled, setTorchEnabled] = useState(false);
  const [reduceMotion, setReduceMotion] = useState<boolean | null>(null);
  const [foreground, setForeground] = useState(
    AppState.currentState !== 'background' &&
      AppState.currentState !== 'inactive',
  );

  useEffect(() => {
    let mounted = true;
    let preferenceChanged = false;
    AccessibilityInfo.isReduceMotionEnabled()
      .then(enabled => {
        if (mounted && !preferenceChanged) {
          setReduceMotion(enabled);
        }
      })
      .catch(() => {
        if (mounted && !preferenceChanged) {
          setReduceMotion(false);
        }
      });
    const subscription = AccessibilityInfo.addEventListener(
      'reduceMotionChanged',
      enabled => {
        preferenceChanged = true;
        if (mounted) {
          setReduceMotion(enabled);
        }
      },
    );
    return () => {
      mounted = false;
      subscription.remove();
    };
  }, []);

  useEffect(() => {
    if (!active) {
      setTorchEnabled(false);
      setHasTorch(false);
    }
  }, [active]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      setForeground(state === 'active');
      if (state !== 'active') {
        setTorchEnabled(false);
        setHasTorch(false);
      }
    });
    return () => {
      subscription.remove();
    };
  }, []);

  const handleScan = useCallback(
    (text: string) => {
      if (active) {
        onScan(text);
      }
    },
    [active, onScan],
  );

  const handleNativeCode = useCallback(
    (event: ScannerCodeEvent) => {
      const text = event?.nativeEvent?.text;
      if (typeof text === 'string') {
        handleScan(text);
      }
    },
    [handleScan],
  );

  const handleNativeState = useCallback((event: ScannerStateEvent) => {
    const nextState = event?.nativeEvent?.state;
    if (typeof nextState !== 'string') {
      return;
    }
    setScannerState(nextState);
    if (nextState === 'running') {
      const nextHasTorch = event?.nativeEvent?.hasTorch === true;
      setHasTorch(nextHasTorch);
      if (!nextHasTorch) {
        setTorchEnabled(false);
      }
    } else {
      setHasTorch(false);
      setTorchEnabled(false);
    }
  }, []);

  const scanPastedText = () => {
    const text = pastedText.trim();
    if (text !== '') {
      handleScan(text);
    }
  };

  const retryScanner = () => {
    setScannerState('starting');
    setHasTorch(false);
    setTorchEnabled(false);
    setScannerKey(value => value + 1);
  };

  const renderPlaceholder = () => (
    <View
      accessibilityLabel={pairAdd.a11y.scanner}
      accessibilityRole="image"
      accessible
      style={[styles.viewfinder, active ? null : styles.inactive]}
      testID={`${testID}-viewfinder`}
    >
      <ViewfinderGradient />
      <ViewfinderGuide animate={false} testID={testID} />
    </View>
  );

  const renderScanner = () => {
    const isRunning = scannerState === 'running';
    const isDenied = scannerState === 'denied' || scannerState === 'restricted';
    const isUnavailable = scannerState === 'unavailable';
    const isError = scannerState === 'error';
    const showTorch = active && foreground && isRunning && hasTorch;

    return (
      <View
        accessibilityLabel={pairAdd.a11y.scanner}
        accessible={false}
        style={[styles.viewfinder, active ? null : styles.inactive]}
        testID={`${testID}-viewfinder`}
      >
        <RsQrScannerView
          key={scannerKey}
          active={active}
          onCode={handleNativeCode}
          onScannerState={handleNativeState}
          style={StyleSheet.absoluteFill}
          torch={showTorch && torchEnabled}
          testID={`${testID}-native`}
        />
        {!isRunning ? <ViewfinderGradient /> : null}
        {isRunning || scannerState === 'starting' ? (
          <ViewfinderGuide
            animate={
              active && foreground && isRunning && reduceMotion === false
            }
            testID={testID}
          />
        ) : null}
        {showTorch ? (
          <Pressable
            accessibilityLabel={
              torchEnabled ? pairAdd.scan.torch.close : pairAdd.scan.torch.open
            }
            accessibilityRole="button"
            onPress={() => {
              setTorchEnabled(value => !value);
            }}
            style={({ pressed }) => [
              styles.torchButton,
              pressed ? styles.torchButtonPressed : null,
            ]}
            testID={`${testID}-torch`}
          >
            <Icon color="text" name="flashlight" size={22} />
          </Pressable>
        ) : null}
        {isDenied ? (
          <View style={styles.stateOverlay}>
            <Icon color="text3" name="scanCode" size={60} />
            <AppText center color="text" variant="button" weight={600}>
              {pairAdd.scan.cameraDenied}
            </AppText>
            <AppText
              center
              color="text2"
              style={styles.hint}
              variant="footnote"
            >
              {pairAdd.scan.cameraDeniedHint}
            </AppText>
            <Button
              onPress={() => {
                Linking.openSettings().catch(() => {});
              }}
              title={pairAdd.scan.openSettings}
              variant="secondary"
            />
          </View>
        ) : null}
        {isUnavailable ? (
          <View style={styles.stateOverlay}>
            <Icon color="text3" name="scanCode" size={60} />
            <AppText
              center
              color="text2"
              style={styles.hint}
              variant="footnote"
            >
              {pairAdd.scan.cameraUnavailable}
            </AppText>
            {onUseCode === undefined ? null : (
              <Button
                onPress={onUseCode}
                testID={`${testID}-use-code`}
                title={pairAdd.scan.useCode}
                variant="secondary"
              />
            )}
          </View>
        ) : null}
        {isError ? (
          <View style={styles.stateOverlay}>
            <Icon color="text3" name="scanCode" size={60} />
            <AppText center color="text" variant="button" weight={600}>
              {pairAdd.scan.cameraError}
            </AppText>
            <Button
              onPress={retryScanner}
              title={pairAdd.scan.retry}
              variant="secondary"
            />
          </View>
        ) : null}
      </View>
    );
  };

  const nativeScannerAvailable =
    (Platform.OS === 'ios' || Platform.OS === 'android') &&
    UIManager.hasViewManagerConfig('RsQrScannerView');

  return (
    <View
      style={styles.container}
      testID={testID}
      {...({ onScan: handleScan } as ScannerSlotHostProps)}
    >
      {nativeScannerAvailable ? renderScanner() : renderPlaceholder()}
      {__DEV__ ? (
        <View style={styles.devTools}>
          <TextField
            label={pairAdd.scan.pasteLabel}
            onChangeText={setPastedText}
            testID={`${testID}-paste`}
            value={pastedText}
          />
          <Button
            align="start"
            onPress={scanPastedText}
            title={pairAdd.scan.recognize}
            variant="secondary"
          />
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignSelf: 'stretch',
    gap: 12,
  },
  viewfinder: {
    width: '100%',
    // Wide screens (iPad) keep the phone's proportions instead of a strip.
    maxWidth: VIEWFINDER_MAX_WIDTH,
    alignSelf: 'center',
    height: VIEWFINDER_HEIGHT,
    borderRadius: 22,
    backgroundColor: VIEWFINDER_GRADIENT_END,
    overflow: 'hidden',
  },
  inactive: {
    opacity: 0.45,
  },
  stateOverlay: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 12,
    paddingHorizontal: 28,
    backgroundColor: 'transparent',
  },
  hint: {
    fontSize: 13,
    lineHeight: 19,
  },
  runningHint: {
    position: 'absolute',
    left: 14,
    right: 14,
    bottom: 14,
    fontSize: 13,
    lineHeight: 19,
  },
  corner: {
    position: 'absolute',
    width: 34,
    height: 34,
    borderColor: tokens.accent,
  },
  cornerTopLeft: {
    top: 26,
    left: 78,
    borderTopWidth: 3,
    borderLeftWidth: 3,
    borderTopLeftRadius: 10,
  },
  cornerTopRight: {
    top: 26,
    right: 78,
    borderTopWidth: 3,
    borderRightWidth: 3,
    borderTopRightRadius: 10,
  },
  cornerBottomLeft: {
    bottom: 46,
    left: 78,
    borderBottomWidth: 3,
    borderLeftWidth: 3,
    borderBottomLeftRadius: 10,
  },
  cornerBottomRight: {
    bottom: 46,
    right: 78,
    borderBottomWidth: 3,
    borderRightWidth: 3,
    borderBottomRightRadius: 10,
  },
  scanLine: {
    position: 'absolute',
    top: SCAN_LINE_TOP,
    left: 88,
    right: 88,
    height: SCAN_LINE_HEIGHT,
    borderRadius: 1,
    backgroundColor: tokens.accent,
    shadowColor: tokens.accent,
    shadowOffset: { width: 0, height: 0 },
    shadowOpacity: 0.85,
    shadowRadius: 8,
    elevation: 6,
  },
  torchButton: {
    position: 'absolute',
    top: 14,
    right: 14,
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(28,35,40,0.8)',
  },
  torchButtonPressed: {
    opacity: 0.8,
  },
  devTools: {
    gap: 8,
  },
});

export default ScannerSlot;
