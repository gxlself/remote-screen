import { fpDisplay } from './fingerprint';

declare const Buffer: {
  from(input: number[] | Uint8Array): {
    toString(encoding: 'base64url'): string;
  };
  alloc(size: number): { toString(encoding: 'base64url'): string };
};

describe('fpDisplay', () => {
  it.each([
    [
      'V1',
      'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
      '000G 40R4 0M30 E209',
    ],
    [
      'V2',
      '__________________________________________8',
      'ZZZZ ZZZZ ZZZZ ZZZZ',
    ],
    [
      'V3',
      '47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU',
      'WERC 8GMR ZGE1 96QV',
    ],
    [
      'all zero',
      'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
      '0000 0000 0000 0000',
    ],
  ])('formats shared vector %s', (_name, value, expected) => {
    expect(fpDisplay(value)).toBe(expected);
  });

  it('ignores bytes after the first ten', () => {
    const v1 = Array.from({ length: 32 }, (_, index) => index);
    const changed = v1.map((value, index) => (index < 10 ? value : 0xff));

    expect(fpDisplay(Buffer.from(changed).toString('base64url'))).toBe(
      fpDisplay(Buffer.from(v1).toString('base64url')),
    );
  });

  it('changes the last character when the lowest bit of byte 9 changes', () => {
    const bytes = Array.from({ length: 32 }, (_, index) => index);
    bytes[9] = 8;

    expect(fpDisplay(Buffer.from(bytes).toString('base64url'))).toBe(
      '000G 40R4 0M30 E208',
    );
  });

  it('uses the Crockford alphabet with four-character groups', () => {
    const shape = /^[0-9A-HJKMNP-TV-Z]{4}( [0-9A-HJKMNP-TV-Z]{4}){3}$/;
    for (let byte = 0; byte <= 0xff; byte += 1) {
      const bytes = Buffer.from(
        Array.from({ length: 32 }, () => byte),
      ).toString('base64url');
      expect(fpDisplay(bytes)).toMatch(shape);
    }
  });

  it.each([
    '',
    'not-base64!',
    'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
    Buffer.alloc(31).toString('base64url'),
  ])('returns an em dash for invalid input %s', value => {
    expect(fpDisplay(value)).toBe('—');
  });

  it('matches the host-ui HOST_FP fixture', () => {
    const hostFp = Buffer.from([
      0x7f,
      0x3a,
      0x91,
      0xc0,
      ...Array.from({ length: 24 }, () => 0),
      0x5e,
      0x2b,
      0xc2,
      0xd4,
    ]);

    expect(fpDisplay(hostFp.toString('base64url'))).toBe('FWX9 3G00 0000 0000');
  });
});
