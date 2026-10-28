const CODE_LENGTH = 6;

export function sanitizeCode(input: string): string {
  return input
    .replace(/[０-９]/g, char =>
      String.fromCharCode(char.charCodeAt(0) - 0xfee0),
    )
    .replace(/[^0-9]/g, '')
    .slice(0, CODE_LENGTH);
}

export function applyKey(code: string, key: string): string {
  const current = sanitizeCode(code);
  if (key === 'Backspace') {
    return current.slice(0, -1);
  }
  if (/^[0-9]$/.test(sanitizeCode(key))) {
    return sanitizeCode(`${current}${key}`);
  }
  return current;
}

export function isCompleteCode(code: string): boolean {
  return /^[0-9]{6}$/.test(code);
}

export function digitsOf(
  code: string,
): [string, string, string, string, string, string] {
  const [
    first = '',
    second = '',
    third = '',
    fourth = '',
    fifth = '',
    sixth = '',
  ] = sanitizeCode(code);
  return [first, second, third, fourth, fifth, sixth];
}
