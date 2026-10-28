import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  open: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: mocks.open }));

import { isTauri, tauriTransport } from './tauri';

describe('tauriTransport', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('forwards invoke and unwraps event payloads', async () => {
    mocks.invoke.mockResolvedValue('7');
    const stop = vi.fn();
    let handler: ((event: { payload: unknown }) => void) | undefined;
    mocks.listen.mockImplementation((_name: string, cb: (event: { payload: unknown }) => void) => {
      handler = cb;
      return Promise.resolve(stop);
    });
    const t = tauriTransport();
    const cb = vi.fn();

    await expect(t.invoke('rscore_call', { name: 'diagProbe', args: ['h'] })).resolves.toBe('7');
    expect(mocks.invoke).toHaveBeenCalledWith('rscore_call', { name: 'diagProbe', args: ['h'] });
    await expect(t.listen('rscore://event', cb)).resolves.toBe(stop);
    handler?.({ payload: { seq: '1', payload: '{}' } });
    expect(cb).toHaveBeenCalledWith({ seq: '1', payload: '{}' });
  });

  it('picks one file and maps cancel to null', async () => {
    const t = tauriTransport();
    mocks.open.mockResolvedValueOnce('/Users/demo/x.rspair').mockResolvedValueOnce(null);

    await expect(t.pickFile('选择配对文件', ['rspair'])).resolves.toBe('/Users/demo/x.rspair');
    await expect(t.pickFile('选择配对文件', ['rspair'])).resolves.toBeNull();
    expect(mocks.open).toHaveBeenCalledWith({
      title: '选择配对文件',
      multiple: false,
      directory: false,
      filters: [{ name: '选择配对文件', extensions: ['rspair'] }],
    });
  });

  it('detects the Tauri webview', () => {
    expect(isTauri()).toBe(false);
  });
});
