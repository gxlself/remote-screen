import { MemoryKv } from './kv';
import { createMmkvKv } from './mmkvKv';

describe('MemoryKv', () => {
  it('sets, gets and deletes string values', () => {
    const kv = new MemoryKv();
    kv.set('name', 'host');
    expect(kv.getString('name')).toBe('host');

    kv.delete('name');
    expect(kv.getString('name')).toBeUndefined();
  });

  it('lists keys', () => {
    const kv = new MemoryKv({ a: '1', b: '2' });
    expect(kv.keys()).toEqual(['a', 'b']);
  });

  it('round-trips JSON values', () => {
    const kv = new MemoryKv();
    kv.setJSON('value', { a: 1 });
    expect(kv.getString('value')).toBe('{"a":1}');
    expect(kv.getJSON('value')).toEqual({ a: 1 });
  });

  it('returns undefined for malformed JSON', () => {
    const kv = new MemoryKv({ broken: '{' });
    expect(kv.getJSON('broken')).toBeUndefined();
  });
});

describe('createMmkvKv', () => {
  it('uses the MMKV mock as a key-value store', () => {
    const kv = createMmkvKv('rs-app-test');
    kv.set('name', 'host');
    expect(kv.getString('name')).toBe('host');

    kv.setJSON('value', { enabled: true });
    expect(kv.getJSON('value')).toEqual({ enabled: true });
    expect(kv.keys()).toContain('value');

    kv.delete('name');
    expect(kv.getString('name')).toBeUndefined();
  });
});
