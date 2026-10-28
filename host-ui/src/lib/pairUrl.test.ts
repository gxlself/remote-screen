import { describe, expect, it } from 'vitest';

import { hostOfAddr, parsePairUrl } from './pairUrl';

const FP = Buffer.from(Uint8Array.from({ length: 32 }, (_, index) => index)).toString('base64url');
const PREFIX = `rsp://pair?v=1&h=h7v2m9q3k8x4tb1a&fp=${FP}&c=482913&n=studio-mac`;
const URL = `${PREFIX}&a=100.64.0.7:7400,192.168.31.20:7400`;

describe('parsePairUrl', () => {
  it('parses every field', () => {
    expect(parsePairUrl(URL)).toEqual({
      v: 1,
      hostId: 'h7v2m9q3k8x4tb1a',
      fp: FP,
      code: '482913',
      name: 'studio-mac',
      addrs: ['100.64.0.7:7400', '192.168.31.20:7400'],
    });
  });

  it('percent-decodes the host name', () => {
    expect(parsePairUrl(URL.replace('n=studio-mac', 'n=%E5%B0%8F%E6%98%8E'))?.name).toBe('小明');
  });

  it('accepts unknown parameters and decodes percent-encoded keys', () => {
    expect(parsePairUrl(`${URL}&future=value&%63=482913`)).toBeNull();
    expect(parsePairUrl(`${URL}&future=value`)?.code).toBe('482913');
  });

  const nineAddresses = Array.from({ length: 9 }, (_, index) => `100.64.0.${index + 1}:7400`).join(
    ',',
  );

  it.each([
    URL.replace('v=1', 'v=2'),
    URL.replace('&c=482913', ''),
    URL.replace('c=482913', 'c=12345'),
    `${URL}&c=482913`,
    URL.replace('a=100.64.0.7:7400,192.168.31.20:7400', 'a='),
    `${PREFIX}&a=${nineAddresses}`,
    URL.replace(FP, FP.slice(0, -1)),
    URL.replace(FP, `${FP}=`),
    'rs://pair/example',
    `${URL}#fragment`,
    `${URL}&broken`,
    `${URL}&future=%ZZ`,
  ])('rejects invalid URL %s', (url) => {
    expect(parsePairUrl(url)).toBeNull();
  });
});

describe('hostOfAddr', () => {
  it.each([
    ['100.64.0.7:7400', '100.64.0.7'],
    ['[fd7a:115c:a1e0::1]:7400', 'fd7a:115c:a1e0::1'],
    ['100.64.0.7', '100.64.0.7'],
  ])('extracts the host from %s', (address, expected) => {
    expect(hostOfAddr(address)).toBe(expected);
  });
});
