import {
  EMPTY_SNIPPETS,
  SNIPPET_MAX,
  addSnippet,
  parseSnippets,
  removeSnippet,
  snippetLabel,
  snippetText,
  updateSnippet,
  validateSnippet,
} from './snippets';
import type { Snippet, SnippetsValue } from './snippets';

function value(items: Snippet[]): SnippetsValue {
  return { v: 1, items };
}

const docker: Snippet = {
  id: 'a',
  label: '',
  cmd: 'docker ps',
  run: true,
};

it('parses valid snippets, drops empty commands, and defaults run to true', () => {
  const parsed = parseSnippets({
    v: 1,
    items: [
      { id: 'a', label: '', cmd: 'docker ps' },
      { id: 'b', cmd: '' },
      { id: 'c', label: 'tail', cmd: 'tail -f api.log', run: false },
    ],
  });

  expect(parsed).toEqual(
    value([
      { id: 'a', label: '', cmd: 'docker ps', run: true },
      { id: 'c', label: 'tail', cmd: 'tail -f api.log', run: false },
    ]),
  );
});

it.each([null, 'x', { v: 2, items: [] }, { v: 1, items: null }])(
  'returns EMPTY_SNIPPETS for malformed input %p',
  raw => {
    expect(parseSnippets(raw)).toEqual(EMPTY_SNIPPETS);
  },
);

it('appends snippets and rejects the fifty-first item', () => {
  const atLimit = value(
    Array.from({ length: SNIPPET_MAX }, (_, index) => ({
      ...docker,
      id: `s${index}`,
    })),
  );

  expect(() =>
    addSnippet(atLimit, { label: 'htop', cmd: 'htop', run: true }, 'sn-new'),
  ).toThrow('full');

  const next = addSnippet(
    value([docker]),
    { label: 'tail', cmd: 'tail -f api.log', run: false },
    'sn-new',
  );
  expect(next.items).toEqual([
    docker,
    { id: 'sn-new', label: 'tail', cmd: 'tail -f api.log', run: false },
  ]);
  expect(value([docker]).items).toEqual([docker]);
});

it('updates and removes snippets by id without changing the original value', () => {
  const original = value([docker, { ...docker, id: 'b', cmd: 'htop' }]);
  const updated = { ...docker, label: 'containers', run: false };

  expect(updateSnippet(original, updated)).toEqual(
    value([updated, { ...docker, id: 'b', cmd: 'htop' }]),
  );
  expect(updateSnippet(original, { ...updated, id: 'missing' })).toBe(original);
  expect(removeSnippet(original, 'a')).toEqual(
    value([{ ...docker, id: 'b', cmd: 'htop' }]),
  );
  expect(removeSnippet(original, 'missing')).toBe(original);
  expect(original.items).toEqual([docker, { ...docker, id: 'b', cmd: 'htop' }]);
});

it.each<[string, string, 'emptyCmd' | 'tooLong' | null]>([
  ['spaces only', '  ', 'emptyCmd'],
  ['empty', '', 'emptyCmd'],
  ['too long', 'x'.repeat(501), 'tooLong'],
  ['at the limit', 'x'.repeat(500), null],
  ['valid', 'docker ps', null],
])('validates a %s command as %p', (_name, cmd, expected) => {
  expect(validateSnippet({ label: '', cmd })).toBe(expected);
});

it('adds a carriage return only when the snippet should run', () => {
  expect(snippetText({ ...docker, run: true })).toBe('docker ps\r');
  expect(snippetText({ ...docker, run: false })).toBe('docker ps');
});

it('uses the label unless blank, otherwise the first command line', () => {
  expect(snippetLabel({ ...docker, label: 'containers' })).toBe('containers');
  expect(
    snippetLabel({ ...docker, label: '  ', cmd: 'tail -f api.log\nhtop' }),
  ).toBe('tail -f api.log');
});
