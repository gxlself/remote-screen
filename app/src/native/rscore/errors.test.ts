import { RsCoreError } from './RsCoreApi';
import { fromNativeId, toNativeId, toRsCoreError } from './errors';

describe('toRsCoreError', () => {
  it('maps HTTP errors and both hub code spellings', () => {
    for (const code of [{ hubCode: 'not_found' }, { hub_code: 'not_found' }]) {
      const error = toRsCoreError({
        tag: 'Http',
        inner: { message: 'nf', status: 404, ...code },
      });
      expect(error).toBeInstanceOf(RsCoreError);
      expect(error).toMatchObject({
        kind: 'http',
        message: 'nf',
        httpStatus: 404,
        hubCode: 'not_found',
      });
    }
  });

  it.each(['currentJson', 'current_json'])('decodes conflict %s', field => {
    const error = toRsCoreError({
      tag: 'Conflict',
      inner: { message: 'c', [field]: '{"version":3,"value":[1]}' },
    });
    expect(error).toMatchObject({
      kind: 'conflict',
      message: 'c',
      httpStatus: 409,
      hubCode: 'conflict',
      current: { version: 3, value: [1] },
    });
    expect(
      toRsCoreError({
        tag: 'Conflict',
        inner: { message: 'c', [field]: 'x' },
      }).current,
    ).toBeUndefined();
  });

  it.each([
    ['Network', 'network'],
    ['Unsupported', 'unsupported'],
    ['Cancelled', 'cancelled'],
    ['Internal', 'internal'],
  ])('maps %s', (tag, kind) => {
    expect(toRsCoreError({ tag, inner: { message: 'm' } })).toMatchObject({
      kind,
      message: 'm',
    });
  });

  it('preserves existing errors and normalizes unknown thrown values', () => {
    const existing = new RsCoreError('network', 'n');
    expect(toRsCoreError(existing)).toBe(existing);
    for (const [value, message] of [
      [new TypeError('t'), 't'],
      ['boom', 'boom'],
      [undefined, 'undefined'],
      [null, 'null'],
      [17, '17'],
      [{ message: 'custom' }, 'custom'],
      [{ tag: 'toString', message: 'unknown tag' }, 'unknown tag'],
    ]) {
      expect(toRsCoreError(value)).toMatchObject({ kind: 'internal', message });
    }
  });
});

describe('native IDs', () => {
  it('preserves u64 precision and allows zero and leading zeros', () => {
    expect(toNativeId('18446744073709551615')).toBe(18446744073709551615n);
    expect(toNativeId('0')).toBe(0n);
    expect(toNativeId('0017')).toBe(17n);
    expect(fromNativeId(17n)).toBe('17');
    expect(fromNativeId(toNativeId('18446744073709551615'))).toBe(
      '18446744073709551615',
    );
  });

  it.each(['-1', '1.5', '18446744073709551616', '', '+1', ' 1', '1e3', '1\n'])(
    'rejects %j',
    value => {
      expect(() => toNativeId(value)).toThrow(RsCoreError);
      expect(() => toNativeId(value)).toThrow(`bad id ${value}`);
    },
  );
});
