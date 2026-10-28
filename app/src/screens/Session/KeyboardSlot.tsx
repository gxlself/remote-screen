import { SessionKeyboard } from '../../native/keyboard/SessionKeyboard';
import type { KeyboardSlotProps } from './slots';

export function KeyboardSlot(p: KeyboardSlotProps) {
  return <SessionKeyboard {...p} />;
}

export default KeyboardSlot;
