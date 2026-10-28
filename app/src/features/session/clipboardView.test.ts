import type { SessionRecord } from './store';
import { clipboardView } from './clipboardView';

const TZ = 480;
function at(hour: number, minute: number): number {
  return Date.UTC(2026, 9, 21, hour - 8, minute, 0) * 1000;
}

function items(): SessionRecord['clipboard'] {
  return [
    { direction: 'fromHost', text: 'a'.repeat(128), at: at(10, 23) },
    { direction: 'fromHost', text: 'second', at: at(10, 18) },
    { direction: 'toHost', text: 'third', at: at(10, 5) },
    { direction: 'fromHost', text: 'fourth', at: at(9, 52) },
  ];
}

it('derives the latest host item, synced metadata and three recent rows', () => {
  const view = clipboardView(items(), true, TZ);
  expect(view.latestFromHost).toEqual({
    text: 'a'.repeat(128),
    chars: 128,
    at: at(10, 23),
  });
  expect(view.meta).toBe('已同步 128 字 · 10:23');
  expect(view.recent).toEqual([
    {
      text: 'second',
      direction: 'fromHost',
      label: '电脑 → 手机 · 10:18',
      a11y: '再次使用：second',
    },
    {
      text: 'third',
      direction: 'toHost',
      label: '手机 → 电脑 · 10:05',
      a11y: '再次使用：third',
    },
    {
      text: 'fourth',
      direction: 'fromHost',
      label: '电脑 → 手机 · 09:52',
      a11y: '再次使用：fourth',
    },
  ]);
});

it('uses paused copy and the Unicode character count', () => {
  const view = clipboardView(
    [{ direction: 'fromHost', text: '😀中a', at: at(10, 23) }],
    false,
    TZ,
  );
  expect(view.meta).toBe('已暂停同步 · 这是 10:23 的');
  expect(view.latestFromHost?.chars).toBe(3);
});

it('returns no latest item for an empty list', () => {
  expect(clipboardView([], true, TZ)).toEqual({
    latestFromHost: null,
    meta: null,
    recent: [],
  });
});

it('falls back to the first record when no host item exists', () => {
  const view = clipboardView(
    [
      { direction: 'toHost', text: 'one', at: at(10, 5) },
      { direction: 'toHost', text: 'two', at: at(10, 4) },
    ],
    true,
    TZ,
  );
  expect(view.latestFromHost).toBeNull();
  expect(view.meta).toBeNull();
  expect(view.recent).toHaveLength(2);
});
