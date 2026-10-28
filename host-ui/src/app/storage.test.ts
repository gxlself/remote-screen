import { afterEach, describe, expect, it, vi } from 'vitest';

import { readFlag, writeFlag } from './storage';

describe('storage flags', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('writes and reads a value', () => {
    writeFlag('rs.test', 'value');
    expect(readFlag('rs.test')).toBe('value');
  });

  it('returns null when reading fails', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('unavailable');
    });
    expect(readFlag('rs.test')).toBeNull();
  });

  it('does not throw when writing fails', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('unavailable');
    });
    expect(() => writeFlag('rs.test', 'value')).not.toThrow();
  });
});
