export type Snippet = {
  id: string;
  label: string;
  cmd: string;
  run: boolean;
};

export type SnippetsValue = { v: 1; items: Snippet[] };

export const EMPTY_SNIPPETS: SnippetsValue = { v: 1, items: [] };
export const SNIPPET_MAX = 50;
export const SNIPPET_CMD_MAX = 500;

type JsonObject = Record<string, unknown>;

function isObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseSnippet(value: unknown): Snippet | null {
  if (
    !isObject(value) ||
    typeof value.id !== 'string' ||
    typeof value.label !== 'string' ||
    typeof value.cmd !== 'string' ||
    value.cmd.trim() === ''
  ) {
    return null;
  }

  return {
    id: value.id,
    label: value.label,
    cmd: value.cmd,
    run: typeof value.run === 'boolean' ? value.run : true,
  };
}

export function parseSnippets(raw: unknown): SnippetsValue {
  if (!isObject(raw) || raw.v !== 1 || !Array.isArray(raw.items)) {
    return EMPTY_SNIPPETS;
  }

  const items: Snippet[] = [];
  for (const item of raw.items) {
    const snippet = parseSnippet(item);
    if (snippet !== null) {
      items.push(snippet);
    }
  }
  return { v: 1, items };
}

export function addSnippet(
  value: SnippetsValue,
  snippet: Omit<Snippet, 'id'>,
  id: string,
): SnippetsValue {
  if (value.items.length >= SNIPPET_MAX) {
    throw new Error('full');
  }
  return { ...value, items: [...value.items, { ...snippet, id }] };
}

export function updateSnippet(
  value: SnippetsValue,
  snippet: Snippet,
): SnippetsValue {
  const index = value.items.findIndex(item => item.id === snippet.id);
  if (index === -1) {
    return value;
  }
  const items = value.items.map((item, itemIndex) =>
    itemIndex === index ? snippet : item,
  );
  return { ...value, items };
}

export function removeSnippet(value: SnippetsValue, id: string): SnippetsValue {
  const items = value.items.filter(item => item.id !== id);
  return items.length === value.items.length ? value : { ...value, items };
}

export function validateSnippet(s: {
  label: string;
  cmd: string;
}): 'emptyCmd' | 'tooLong' | null {
  const cmd = s.cmd.trim();
  if (cmd === '') {
    return 'emptyCmd';
  }
  if (cmd.length > SNIPPET_CMD_MAX) {
    return 'tooLong';
  }
  return null;
}

export function snippetText(snippet: Snippet): string {
  return snippet.run ? `${snippet.cmd}\r` : snippet.cmd;
}

export function snippetLabel(snippet: Snippet): string {
  if (snippet.label.trim() !== '') {
    return snippet.label;
  }
  return snippet.cmd.split(/\r?\n/, 1)[0] ?? snippet.cmd;
}
