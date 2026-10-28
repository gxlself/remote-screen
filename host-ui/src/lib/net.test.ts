import { describe, expect, it } from 'vitest';

import { firstIPv4, isIPv4, isValidCidr } from './net';

describe('isIPv4', () => {
  it.each(['0.0.0.0', '192.168.31.20', '255.255.255.255'])('accepts %s', (address) => {
    expect(isIPv4(address)).toBe(true);
  });

  it.each(['256.1.1.1', '010.1.1.1', '1.2.3', '1.2.3.4.5', '1.2.3.-1', '1.2.3.4\n', ''])(
    'rejects %s',
    (address) => {
      expect(isIPv4(address)).toBe(false);
    },
  );
});

describe('isValidCidr', () => {
  it.each(['10.30.0.0/16', '192.168.31.0/24', ' 10.30.12.0/24 ', 'fd7a:115c:a1e0::/48', '::/0'])(
    'accepts %s',
    (cidr) => {
      expect(isValidCidr(cidr)).toBe(true);
    },
  );

  it.each([
    '10.30.0.0',
    '10.30.0.0/33',
    '256.1.1.1/8',
    '010.1.1.1/8',
    'abc/8',
    'fd7a::1::2/64',
    '',
    'fd7a:115c:a1e0::/129',
  ])('rejects %s', (cidr) => {
    expect(isValidCidr(cidr)).toBe(false);
  });
});

describe('firstIPv4', () => {
  it('returns the first IPv4 string in an array', () => {
    expect(firstIPv4(['fe80::1', '192.168.31.20'])).toBe('192.168.31.20');
  });

  it('returns null for non-arrays', () => {
    expect(firstIPv4('x')).toBeNull();
  });
});
