import { describe, expect, it } from 'vitest';

import { pathLabel, permissionLabel } from './labels';

describe('pathLabel', () => {
  it('translates known paths', () => {
    expect(pathLabel('tailnet_derp')).toBe('中继');
    expect(pathLabel('tailnet_direct')).toBe('直连');
    expect(pathLabel('intranet')).toBe('内网');
    expect(pathLabel('lan')).toBe('局域网');
  });

  it('leaves unknown paths unchanged', () => {
    expect(pathLabel('x')).toBe('x');
  });

  it('renders nullish paths as an empty string', () => {
    expect(pathLabel(null)).toBe('');
    expect(pathLabel(undefined)).toBe('');
  });
});

describe('permissionLabel', () => {
  it('translates known permissions', () => {
    expect(permissionLabel('view_only')).toBe('仅观看');
    expect(permissionLabel('control')).toBe('允许控制');
    expect(permissionLabel('unattended')).toBe('无人值守');
  });

  it('leaves unknown permissions unchanged', () => {
    expect(permissionLabel('own')).toBe('own');
  });
});
