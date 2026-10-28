import { ipadSession } from '../../i18n/zh/ipadSession';
import type { SessionCombo } from '../../native/rscore/types';

export type IpadKey = {
  id: string;
  label: string;
  a11y: string;
  combo: SessionCombo | null;
};

type KeyId = Exclude<keyof typeof ipadSession.keys, 'globe'>;

function comboKey(id: KeyId & SessionCombo): IpadKey {
  return {
    id,
    label: ipadSession.keys[id].label,
    a11y: ipadSession.keys[id].a11y,
    combo: id,
  };
}

/** Shortcuts iPadOS keeps for itself, offered as buttons instead. */
export function ipadKeysFor(os: 'macos' | 'windows'): IpadKey[] {
  if (os === 'windows') {
    return [comboKey('altTab'), comboKey('winKey'), comboKey('winSpace')];
  }

  return [
    comboKey('cmdTab'),
    comboKey('cmdSpace'),
    comboKey('ctrlSpace'),
    comboKey('cmdH'),
    comboKey('cmdShift4'),
    {
      id: 'globe',
      label: ipadSession.keys.globe.label,
      a11y: ipadSession.keys.globe.a11y,
      combo: null,
    },
  ];
}
