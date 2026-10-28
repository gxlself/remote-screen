import { needsSessionAuth } from './auth';

const HOST_ID = 'h7v2m9q3k8x4tb1a';

it.each([
  ['global lock and non-sensitive host', false, [], false],
  ['global lock enabled', true, [], true],
  ['sensitive host', false, [HOST_ID], true],
  ['global lock and sensitive host', true, [HOST_ID], true],
  ['another sensitive host', false, ['another-host'], false],
] satisfies [string, boolean, string[], boolean][])(
  '%s',
  (_name, beforeSession, sensitiveHosts, expected) => {
    expect(needsSessionAuth(HOST_ID, { beforeSession, sensitiveHosts })).toBe(
      expected,
    );
  },
);
