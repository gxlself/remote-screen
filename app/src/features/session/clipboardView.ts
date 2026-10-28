import { formatHm } from '../format/time';
import { clipboardMore } from '../../i18n/zh/clipboardMore';
import type { SessionRecord } from './store';

export type ClipboardViewItem = {
  text: string;
  direction: 'fromHost' | 'toHost';
  label: string;
  a11y: string;
};

export type ClipboardView = {
  latestFromHost: { text: string; chars: number; at: number } | null;
  meta: string | null;
  recent: ClipboardViewItem[];
};

const RECENT_LIMIT = 3;
const PREVIEW_LIMIT = 40;

function charCount(text: string): number {
  return Array.from(text).length;
}

function preview(text: string): string {
  return Array.from(text).slice(0, PREVIEW_LIMIT).join('');
}

export function latestFromHostIndex(items: SessionRecord['clipboard']): number {
  return items.findIndex(item => item.direction === 'fromHost');
}

export function clipboardView(
  items: SessionRecord['clipboard'],
  autoSync: boolean,
  tzOffsetMin?: number,
): ClipboardView {
  const copy = clipboardMore.clipboard;
  const latestIndex = latestFromHostIndex(items);
  const latest = latestIndex < 0 ? null : items[latestIndex];
  const latestFromHost =
    latest === null
      ? null
      : {
          text: latest.text,
          chars: charCount(latest.text),
          at: latest.at,
        };
  const meta =
    latest === null
      ? null
      : autoSync
      ? copy.synced(
          latestFromHost?.chars ?? 0,
          formatHm(latest.at, tzOffsetMin),
        )
      : copy.paused(formatHm(latest.at, tzOffsetMin));
  const recent = items
    .filter((_, index) => index !== latestIndex)
    .slice(0, RECENT_LIMIT)
    .map(item => ({
      text: item.text,
      direction: item.direction,
      label: copy.recentLabel(item.direction, formatHm(item.at, tzOffsetMin)),
      a11y: copy.reuseA11y(preview(item.text)),
    }));

  return { latestFromHost, meta, recent };
}
