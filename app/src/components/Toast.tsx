import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import { ripple } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon, type IconName } from './icons/Icon';

export type Tone = 'neutral' | 'good' | 'bad';

export type ToastAction = {
  label: string;
  onPress: () => void;
};

export type ToastState = {
  message: string;
  tone?: Tone;
  icon?: IconName;
  action?: ToastAction;
};

export type ToastOptions = {
  tone?: Tone;
  icon?: IconName;
  action?: ToastAction;
};

export type UseToastResult = {
  toast: ToastState | null;
  show: (message: string, opts?: ToastOptions) => void;
  hide: () => void;
};

export type ToastProps = {
  toast: ToastState | null;
  onHide: () => void;
  position?: 'top' | 'bottom';
  offset?: number;
  testID?: string;
};

export const TOAST_DURATION_MS = 3000;
// A toast with an action (撤销) stays long enough to reach the button.
export const TOAST_ACTION_DURATION_MS = 6000;

export function toastDuration(
  toast: Pick<ToastState, 'action'>,
  durationMs = TOAST_DURATION_MS,
): number {
  return toast.action == null
    ? durationMs
    : Math.max(durationMs, TOAST_ACTION_DURATION_MS);
}

const defaultIcons: Record<Tone, IconName | undefined> = {
  neutral: undefined,
  good: 'check',
  bad: 'alert',
};

const toneColors: Record<Tone, keyof typeof tokens> = {
  neutral: 'text',
  good: 'good',
  bad: 'bad',
};

export function useToast(durationMs = TOAST_DURATION_MS): UseToastResult {
  const [toast, setToast] = useState<ToastState | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = useCallback(() => {
    if (timer.current != null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const hide = useCallback(() => {
    clearTimer();
    setToast(null);
  }, [clearTimer]);

  const show = useCallback(
    (message: string, opts?: ToastOptions) => {
      clearTimer();
      const next: ToastState = {
        message,
        tone: opts?.tone,
        icon: opts?.icon,
        action: opts?.action,
      };
      setToast(next);
      timer.current = setTimeout(() => {
        timer.current = null;
        setToast(null);
      }, toastDuration(next, durationMs));
    },
    [clearTimer, durationMs],
  );

  useEffect(() => clearTimer, [clearTimer]);

  return { toast, show, hide };
}

export function Toast({
  toast,
  onHide,
  position = 'bottom',
  offset,
  testID = 'toast',
}: ToastProps) {
  const platform = Platform.OS;
  const isAndroid = platform === 'android';
  const resolvedOffset = offset ?? (position === 'top' ? 12 : 100);
  const tone = toast?.tone ?? 'neutral';
  const icon = toast?.icon ?? defaultIcons[tone];
  const message = toast?.message ?? '';
  const action = toast?.action;

  useEffect(() => {
    if (toast != null && platform === 'ios') {
      AccessibilityInfo.announceForAccessibility(toast.message);
    }
  }, [platform, toast]);

  if (toast == null) {
    return null;
  }

  return (
    <View
      pointerEvents={action == null ? 'none' : 'box-none'}
      style={[
        styles.layer,
        position === 'top'
          ? { top: resolvedOffset }
          : { bottom: resolvedOffset },
      ]}
    >
      <View
        // With an action the message and the button are separate stops.
        {...(action == null
          ? { accessible: true, accessibilityLabel: message }
          : {})}
        accessibilityLiveRegion="polite"
        style={[styles.toast, isAndroid ? styles.androidSnackbar : null]}
        testID={testID}
      >
        {icon == null ? null : (
          <Icon color={toneColors[tone]} name={icon} size={16} />
        )}
        <AppText
          color={toneColors[tone]}
          style={[styles.message, isAndroid ? styles.androidMessage : null]}
          variant="footnote"
          weight={500}
        >
          {message}
        </AppText>
        {action == null ? null : (
          <ToastActionButton
            action={action}
            onHide={onHide}
            testID={`${testID}-action`}
          />
        )}
      </View>
    </View>
  );
}

function ToastActionButton({
  action,
  onHide,
  testID,
}: {
  action: ToastAction;
  onHide: () => void;
  testID: string;
}) {
  const android = Platform.OS === 'android';
  const minHeight = android ? hit.min : 32;
  const slop = (hit.min - minHeight) / 2;
  return (
    <Pressable
      accessibilityLabel={action.label}
      accessibilityRole="button"
      android_ripple={
        android ? { color: ripple.accent, foreground: true } : undefined
      }
      hitSlop={{ top: slop, bottom: slop }}
      onPress={() => {
        onHide();
        action.onPress();
      }}
      style={({ pressed }) => [
        styles.action,
        { minHeight },
        pressed && !android ? styles.actionPressed : null,
      ]}
      testID={testID}
    >
      <AppText color="accent" variant="button">
        {action.label}
      </AppText>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  // Spans the container so the toast centres in it, whatever its padding.
  layer: {
    position: 'absolute',
    left: 0,
    right: 0,
    alignItems: 'center',
    paddingHorizontal: 12,
  },
  toast: {
    minHeight: 40,
    maxWidth: 480,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 20,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.hud,
  },
  // 同 07 · Android：Snackbar，沿用 HUD 颜色 token。
  androidSnackbar: {
    borderRadius: 4,
    minHeight: 48,
    width: '100%',
    maxWidth: 560,
  },
  message: {
    flexShrink: 1,
  },
  androidMessage: {
    flexGrow: 1,
    textAlign: 'left',
  },
  action: {
    justifyContent: 'center',
    marginLeft: 4,
    paddingHorizontal: 8,
    borderRadius: 8,
  },
  actionPressed: {
    opacity: 0.6,
  },
});

export default Toast;
