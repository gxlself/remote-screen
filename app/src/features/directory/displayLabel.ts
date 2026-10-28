import { deviceSheet } from '../../i18n/zh/deviceSheet';
import type { HostDisplay } from '../../native/rscore/types';

export function displayTitle(index: number): string {
  return deviceSheet.displays.label(index);
}

/** The display's own name; "显示器 N" by position when it has none. */
export function displayName(display: HostDisplay, position: number): string {
  const name = display.name.trim();
  return name === '' ? displayTitle(position) : name;
}

export function displayDetail(display: HostDisplay): string {
  const resolution = `${display.widthPx}×${display.heightPx}`;
  const mode = display.primary
    ? deviceSheet.displays.primary
    : display.heightPx > display.widthPx
    ? deviceSheet.displays.portrait
    : null;

  return mode === null ? resolution : `${resolution} · ${mode}`;
}

export function sortDisplays(list: readonly HostDisplay[]): HostDisplay[] {
  return [...list].sort((left, right) => {
    if (left.primary !== right.primary) {
      return left.primary ? -1 : 1;
    }
    return left.id - right.id;
  });
}
