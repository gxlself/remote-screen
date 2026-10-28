import { formatPairAddress, parsePairAddress } from './address';

describe('parsePairAddress', () => {
  it.each([
    ['100.64.0.3', { addr: '100.64.0.3' }],
    ['100.64.0.3:7401', { addr: '100.64.0.3', port: 7401 }],
    ['home-mac', { addr: 'home-mac' }],
    ['home-mac:7400', { addr: 'home-mac', port: 7400 }],
    ['home-mac.tail1234.ts.net', { addr: 'home-mac.tail1234.ts.net' }],
    ['fd7a::1', { addr: 'fd7a::1' }],
    ['[fd7a::1]', { addr: 'fd7a::1' }],
    ['[fd7a::1]:7400', { addr: 'fd7a::1', port: 7400 }],
    ['  home-mac : 7400 ', { addr: 'home-mac', port: 7400 }],
  ])('parses %j', (text, expected) => {
    expect(parsePairAddress(text)).toEqual(expected);
  });

  it.each([
    '',
    '   ',
    '1.2.3.256',
    'mac_gxl',
    'home-mac:0',
    'home-mac:70000',
    'home-mac:',
    'a:b:c',
    '[home-mac]',
    '[fd7a::1]:',
    'http://home-mac',
  ])('rejects %j', text => {
    expect(parsePairAddress(text)).toBeNull();
  });

  it('keeps no port key when none was typed', () => {
    expect(parsePairAddress('home-mac')).not.toHaveProperty('port');
  });
});

describe('formatPairAddress', () => {
  it('formats route params back into the field text', () => {
    expect(formatPairAddress()).toBe('');
    expect(formatPairAddress('home-mac')).toBe('home-mac');
    expect(formatPairAddress('home-mac', 7400)).toBe('home-mac:7400');
    expect(formatPairAddress('fd7a::1', 7400)).toBe('[fd7a::1]:7400');
    expect(parsePairAddress(formatPairAddress('fd7a::1', 7400))).toEqual({
      addr: 'fd7a::1',
      port: 7400,
    });
  });
});
