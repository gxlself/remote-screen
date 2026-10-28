import { deviceSheet } from '../../i18n/zh/deviceSheet';
import type { HostDisplay } from '../../native/rscore/types';
import {
  displayDetail,
  displayName,
  displayTitle,
  sortDisplays,
} from './displayLabel';

const display = (patch: Partial<HostDisplay>): HostDisplay => ({
  id: 1,
  name: 'Display',
  widthPx: 1920,
  heightPx: 1080,
  primary: false,
  ...patch,
});

describe('displayLabel', () => {
  it('formats the display title', () => {
    expect(displayTitle(1)).toBe('显示器 1');
    expect(displayTitle(3)).toBe('显示器 3');
  });

  it.each([
    [
      display({ widthPx: 2560, heightPx: 1600, primary: true }),
      '2560×1600 · 主显示器',
    ],
    [
      display({ widthPx: 2160, heightPx: 3840, primary: false }),
      '2160×3840 · 竖放',
    ],
    [display({ widthPx: 1920, heightPx: 1080, primary: false }), '1920×1080'],
  ])('formats display details', (value, expected) => {
    expect(displayDetail(value)).toBe(expected);
  });

  it('prefers the primary marker when a primary display is portrait', () => {
    expect(
      displayDetail(display({ widthPx: 2160, heightPx: 3840, primary: true })),
    ).toBe(`2160×3840 · ${deviceSheet.displays.primary}`);
  });

  it('sorts the primary display first and the rest by id without mutating', () => {
    const input = [
      display({ id: 9 }),
      display({ id: 3, primary: true }),
      display({ id: 2 }),
      display({ id: 7 }),
    ];

    expect(sortDisplays(input).map(item => item.id)).toEqual([3, 2, 7, 9]);
    expect(input.map(item => item.id)).toEqual([9, 3, 2, 7]);
  });

  it('names a display by what the computer reports, else by position', () => {
    expect(displayName(display({ name: 'Studio Display' }), 1)).toBe(
      'Studio Display',
    );
    expect(displayName(display({ name: '  竖屏  ' }), 2)).toBe('竖屏');
    expect(displayName(display({ name: '' }), 2)).toBe('显示器 2');
    expect(displayName(display({ name: '   ' }), 3)).toBe('显示器 3');
  });
});
