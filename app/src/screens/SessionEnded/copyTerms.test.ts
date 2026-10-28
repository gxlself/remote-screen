import { adapters } from '../../i18n/zh/adapters';
import { errBoards1 } from '../../i18n/zh/errBoards1';
import { errBoards2 } from '../../i18n/zh/errBoards2';
import { errBoards3 } from '../../i18n/zh/errBoards3';
import { fallback } from '../../i18n/zh/fallback';
import { sessionEnded } from '../../i18n/zh/sessionEnded';

// Every argument a copy function in these files is called with in the App:
// systems, protocols, adapter error codes, names, numbers and flags.
const ARGS: readonly unknown[] = [
  'macos',
  'windows',
  null,
  'vnc',
  'rdp',
  'refused',
  'timeout',
  'authFailed',
  'unsupportedSecurity',
  'nlaRequired',
  'certRejected',
  'serverClosed',
  'io',
  'Studio',
  12,
  true,
  false,
];

function argLists(arity: number): unknown[][] {
  if (arity === 0) {
    return [[]];
  }
  return argLists(arity - 1).flatMap(rest => ARGS.map(arg => [arg, ...rest]));
}

function collect(value: unknown, path: string, out: [string, string][]) {
  if (typeof value === 'string') {
    out.push([path, value]);
  } else if (typeof value === 'function') {
    for (const args of argLists(Math.min(value.length, 3))) {
      let result: unknown;
      try {
        result = (value as (...a: unknown[]) => unknown)(...args);
      } catch {
        continue;
      }
      collect(result, path, out);
    }
  } else if (Array.isArray(value)) {
    value.forEach((item, index) => collect(item, `${path}[${index}]`, out));
  } else if (value !== null && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      collect(item, `${path}.${key}`, out);
    }
  }
}

// V48, V59, UX-ROUND: user-facing copy names Tailscale, the computer and
// Host; no protocol names, error codes, Hub or developer notes.
const BANNED =
  /RS-E\d|RSP\/1|tailnet|VNC App|Remote Host|\bHub\b|TLS|WhoIs|实测|拷贝/;

it.each([
  ['sessionEnded', sessionEnded],
  ['errBoards1', errBoards1],
  ['errBoards2', errBoards2],
  ['errBoards3', errBoards3],
  ['adapters', adapters],
  ['fallback', fallback],
])('%s copy has no jargon or error codes', (name, copy) => {
  const strings: [string, string][] = [];
  collect(copy, name, strings);
  expect(strings.length).toBeGreaterThan(5);
  expect(strings.filter(([, text]) => BANNED.test(text))).toEqual([]);
});

it('catches a banned term', () => {
  const strings: [string, string][] = [];
  collect(
    { a: (n: number) => `RS-E${n}`, b: 'tailnet 在线' },
    'probe',
    strings,
  );
  const caught = strings.filter(([, text]) => BANNED.test(text));
  expect(new Set(caught.map(([path]) => path))).toEqual(
    new Set(['probe.a', 'probe.b']),
  );
});
