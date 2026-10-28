import { describe, expect, it } from 'vitest';

import { zh } from './zh';

describe('zh', () => {
  it('has all 14 namespaces', () => {
    expect(Object.keys(zh)).toEqual([
      'common',
      'menu',
      'capsule',
      'confirm',
      'pair',
      'devices',
      'settings',
      'permissions',
      'history',
      'network',
      'pairingMethods',
      'setup',
      'update',
      'bg',
    ]);
  });

  it('has non-empty common strings', () => {
    const check = (value: unknown): void => {
      if (typeof value === 'string') {
        expect(value.trim().length).toBeGreaterThan(0);
      } else if (value !== null && typeof value === 'object') {
        Object.values(value).forEach(check);
      }
    };
    Object.values(zh.common).forEach(check);
  });
});
