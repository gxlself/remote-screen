import { Platform } from 'react-native';
import { Dialog } from '../../components/Dialog';
import { fallback } from '../../i18n/zh/fallback';
import { usePlatform } from '../../native/platform/PlatformContext';
import { openFirst } from '../platform/externalApps';
import { storeUrls } from './remoteFallback';
import type { FallbackGuideState } from './useRemoteFallback';

export type FallbackGuideProps = {
  guide: FallbackGuideState;
  onClose: () => void;
};

function readIsPad(platform: ReturnType<typeof usePlatform>): boolean {
  try {
    return platform.deviceInfo().isPad;
  } catch {
    return false;
  }
}

export function FallbackGuide({ guide, onClose }: FallbackGuideProps) {
  const platform = usePlatform();

  if (guide === null) {
    return null;
  }

  const copy = fallback.guide[guide.kind];
  const message =
    guide.kind !== 'vnc'
      ? copy.message
      : Platform.OS === 'android'
      ? fallback.guide.vnc.messageAndroid
      : readIsPad(platform)
      ? fallback.guide.vnc.messagePad
      : copy.message;
  const storeLabel =
    Platform.OS === 'android'
      ? fallback.actions.storeAndroid
      : fallback.actions.store;
  const openStore = () => {
    openFirst(platform, storeUrls(guide.kind)).catch(() => undefined);
    onClose();
  };

  return (
    <Dialog
      actions={[
        { label: storeLabel, onPress: openStore },
        {
          label: fallback.actions.cancel,
          onPress: onClose,
          variant: 'secondary',
        },
      ]}
      message={message}
      onDismiss={onClose}
      testID="fallback-guide"
      title={copy.title}
      visible
    />
  );
}
