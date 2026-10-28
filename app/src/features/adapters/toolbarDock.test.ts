import { MemoryKv } from '../storage/kv';
import { readToolbarDock, TOOLBAR_DOCK_KEY } from './toolbarDock';

it('reads the saved edge and falls back to the bottom', () => {
  const kv = new MemoryKv();
  expect(readToolbarDock(kv)).toBe('bottom');
  kv.setJSON(TOOLBAR_DOCK_KEY, 'right');
  expect(readToolbarDock(kv)).toBe('right');
  kv.setJSON(TOOLBAR_DOCK_KEY, 'top');
  expect(readToolbarDock(kv)).toBe('bottom');
});
