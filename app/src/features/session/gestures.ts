import type { IconName } from '../../components/icons/Icon';
import { coach } from '../../i18n/zh/coach';

export type CoachMode = 'trackpad' | 'touch';
export type CoachOs = 'macos' | 'windows';

export type CoachItem = {
  gesture: string;
  effect: string;
  icon: IconName;
};

export type CoachContent = {
  title: string;
  body: string;
  items: CoachItem[];
  footer: string[];
  switchLabel: string;
  switchTo: CoachMode;
};

function items(
  source: (typeof coach)[CoachMode]['items'],
  icons: readonly IconName[],
): CoachItem[] {
  return Object.values(source).map((item, index) => ({
    ...item,
    icon: icons[index],
  }));
}

const trackpadIcons = [
  'pointer',
  'pointer',
  'hand',
  'swipeHand',
  'zoomIn',
  'keyboard',
  'swipeHand',
] as const satisfies readonly IconName[];

const touchIcons = [
  'hand',
  'pointer',
  'hand',
  'swipeHand',
  'zoomIn',
  'keyboard',
  'swipeHand',
] as const satisfies readonly IconName[];

export function coachContent(mode: CoachMode, os: CoachOs): CoachContent {
  if (mode === 'touch') {
    return {
      title: coach.touch.title,
      body: coach.touch.body,
      items: items(coach.touch.items, touchIcons),
      footer: [...coach.touch.footer],
      switchLabel: coach.touch.switchLabel,
      switchTo: 'trackpad',
    };
  }
  return {
    title: coach.trackpad.title,
    body: coach.trackpad.body(os === 'macos'),
    items: items(coach.trackpad.items, trackpadIcons),
    footer: [...coach.trackpad.footer],
    switchLabel: coach.trackpad.switchLabel,
    switchTo: 'touch',
  };
}
