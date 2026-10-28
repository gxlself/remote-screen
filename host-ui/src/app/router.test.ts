import { describe, expect, it } from 'vitest';

import { buildHash, navigate, parseHash, type Route } from './router';

describe('parseHash', () => {
  it.each([
    ['#/menu', { page: 'menu', query: {} }],
    ['#/settings/devices', { page: 'settings', section: 'devices', query: {} }],
    ['#/settings', { page: 'settings', query: {} }],
    ['#/pair?scenario=x&y=1', { page: 'pair', query: { scenario: 'x', y: '1' } }],
    ['#/settings/devices/extra', { page: 'settings', section: 'devices', query: {} }],
  ] as const)('parses %s', (hash, expected) => {
    expect(parseHash(hash)).toEqual(expected);
  });

  it.each(['', '#/', '#/nope'])('falls back to dev for %s', (hash) => {
    expect(parseHash(hash)).toEqual({ page: 'dev', query: {} });
  });
});

describe('buildHash', () => {
  const routes: Route[] = [
    { page: 'menu', query: {} },
    { page: 'settings', section: 'devices', query: {} },
    { page: 'settings', query: {} },
    { page: 'pair', query: { scenario: 'x', y: '1' } },
    { page: 'dev', query: {} },
  ];

  it.each(routes)('round-trips %o', (route) => {
    expect(parseHash(buildHash(route))).toEqual(route);
  });
});

describe('navigate', () => {
  it('updates location.hash', () => {
    navigate('pair');
    expect(location.hash).toBe('#/pair');
  });
});
