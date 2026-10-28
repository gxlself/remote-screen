const BASE64URL_ALPHABET =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
const CROCKFORD_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

function decodeBase64Url(value: string): Uint8Array | null {
  if (value.length === 0 || value.length % 4 === 1) {
    return null;
  }

  const bytes: number[] = [];
  const values: number[] = [];

  for (const character of value) {
    const digit = BASE64URL_ALPHABET.indexOf(character);
    if (digit < 0) {
      return null;
    }
    values.push(digit);
  }

  const completeGroups = Math.floor(values.length / 4);
  for (let group = 0; group < completeGroups; group += 1) {
    const first = values[group * 4];
    const second = values[group * 4 + 1];
    const third = values[group * 4 + 2];
    const fourth = values[group * 4 + 3];
    bytes.push(
      first * 4 + Math.floor(second / 16),
      (second % 16) * 16 + Math.floor(third / 4),
      (third % 4) * 64 + fourth,
    );
  }

  const remainder = values.length % 4;
  if (remainder === 1) {
    return null;
  }
  if (remainder === 2) {
    const first = values[values.length - 2];
    const second = values[values.length - 1];
    if (second % 16 !== 0) {
      return null;
    }
    bytes.push(first * 4 + Math.floor(second / 16));
  } else if (remainder === 3) {
    const first = values[values.length - 3];
    const second = values[values.length - 2];
    const third = values[values.length - 1];
    if (third % 4 !== 0) {
      return null;
    }
    bytes.push(
      first * 4 + Math.floor(second / 16),
      (second % 16) * 16 + Math.floor(third / 4),
    );
  }
  return bytes.length === 32 ? Uint8Array.from(bytes) : null;
}

function encodeCrockfordBase32(bytes: Uint8Array): string {
  let buffer = 0;
  let bufferedBits = 0;
  let encoded = '';

  for (let index = 0; index < 10; index += 1) {
    buffer = buffer * 256 + bytes[index];
    bufferedBits += 8;

    while (bufferedBits >= 5) {
      bufferedBits -= 5;
      encoded +=
        CROCKFORD_ALPHABET[
          Math.floor(buffer / 2 ** bufferedBits) % CROCKFORD_ALPHABET.length
        ];
    }
    buffer %= 2 ** bufferedBits;
  }

  return encoded;
}

function groupCharacters(value: string): string {
  const groups: string[] = [];
  for (let offset = 0; offset < value.length; offset += 4) {
    groups.push(value.slice(offset, offset + 4));
  }
  return groups.join(' ');
}

export function fpDisplay(fp: string): string {
  const bytes = decodeBase64Url(fp);
  if (bytes === null) {
    return '—';
  }

  return groupCharacters(encodeCrockfordBase32(bytes));
}
