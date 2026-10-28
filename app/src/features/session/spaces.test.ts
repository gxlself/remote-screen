import {
  clearArrival,
  findScreen,
  markArrival,
  peekArrival,
  resetArrival,
  spaceDirection,
  spacePosition,
  spacesOf,
} from './spaces';
import { buildSwitcherList } from './switcher';
import type { SwitcherItem } from './switcher';

function screen(id: string, startedAtUs: number): SwitcherItem {
  return {
    kind: 'vnc',
    id,
    params: { target: { protocol: 'vnc', addr: `10.0.0.${id}` } },
    name: `Mac ${id}`,
    status: 'connected',
    startedAtUs,
  };
}

const terminal: SwitcherItem = {
  kind: 'ssh',
  id: 't',
  serverId: 'srv',
  name: 'NAS',
  status: 'connected',
  startedAtUs: 1,
};

afterEach(() => {
  resetArrival();
});

it('lines up the full-screen sessions in the switcher order, terminals left out', () => {
  const list = buildSwitcherList([screen('3', 30), terminal, screen('1', 10)]);
  expect(spacesOf(list)).toEqual([
    { id: '1', name: 'Mac 1' },
    { id: '3', name: 'Mac 3' },
  ]);
  expect(findScreen(list, '3')?.name).toBe('Mac 3');
  expect(findScreen(list, 't')).toBeUndefined();
});

it('knows which way a session lies and where it sits', () => {
  const spaces = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
  expect(spaceDirection(spaces, 'a', 'b')).toBe('next');
  expect(spaceDirection(spaces, 'a', 'c')).toBe('next');
  expect(spaceDirection(spaces, 'c', 'a')).toBe('previous');
  expect(spaceDirection(spaces, 'b', 'b')).toBeNull();
  expect(spaceDirection(spaces, null, 'b')).toBeNull();
  expect(spaceDirection(spaces, 'a', 'z')).toBeNull();
  expect(spacePosition(spaces, 'b')).toEqual({ index: 1, count: 3 });
  expect(spacePosition(spaces, 'z')).toBeNull();
});

it('remembers a switch for the page it opens, for a while', () => {
  markArrival('7', 1_000);
  expect(peekArrival('7', 1_500)).toBe(true);
  // Peeking does not use it up (StrictMode renders twice).
  expect(peekArrival('7', 1_600)).toBe(true);
  expect(peekArrival('8', 1_600)).toBe(false);
  expect(peekArrival('7', 11_000)).toBe(false);
  clearArrival('8');
  expect(peekArrival('7', 1_700)).toBe(true);
  clearArrival('7');
  expect(peekArrival('7', 1_700)).toBe(false);
});
