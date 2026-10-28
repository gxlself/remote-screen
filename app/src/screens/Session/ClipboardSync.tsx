import { useEffect, useRef } from 'react';
import { useSetting } from '../../features/settings/store';
import type { SessionRecord } from '../../features/session/store';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { SlotProps } from './slots';

type ClipboardEntry = SessionRecord['clipboard'][number];

function isNewFromHost(
  entry: ClipboardEntry | undefined,
  previous: ClipboardEntry | undefined,
): entry is ClipboardEntry {
  return (
    entry !== undefined &&
    entry.direction === 'fromHost' &&
    (previous === undefined || entry !== previous || entry.at !== previous.at)
  );
}

export function ClipboardSync({ ctx }: SlotProps) {
  const platform = usePlatform();
  const [autoSync] = useSetting('clipboard.autoSync');
  const head = ctx.rec?.clipboard[0];
  const seen = useRef<ClipboardEntry | undefined>(head);
  const mounted = useRef(false);
  const initialized = useRef(ctx.rec !== undefined);

  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      seen.current = head;
      return;
    }
    if (!initialized.current) {
      if (ctx.rec === undefined) {
        return;
      }
      initialized.current = true;
      seen.current = head;
      return;
    }
    const previous = seen.current;
    seen.current = head;
    if (isNewFromHost(head, previous) && autoSync) {
      platform.copyText(head.text).catch(() => {});
    }
  }, [autoSync, ctx.rec, head, platform]);

  return null;
}

export default ClipboardSync;
