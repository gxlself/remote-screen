import { CONTROL_URL_MAX, normalizeControlUrl } from './controlUrl';

describe('normalizeControlUrl', () => {
  it.each([
    [' hs.example.com ', 'https://hs.example.com'],
    ['https://hs.example.com/', 'https://hs.example.com'],
    ['HTTPS://HS.Example.com/base/', 'https://hs.example.com/base'],
    ['http://192.168.31.5:8080', 'http://192.168.31.5:8080'],
    ['hs.example.com:1/Base///', 'https://hs.example.com:1/Base'],
    ['hs.example.com:65535', 'https://hs.example.com:65535'],
    [
      'https://[fd7a:115c:a1e0::15]:8443/base/',
      'https://[fd7a:115c:a1e0::15]:8443/base',
    ],
    ['', null],
    ['   ', null],
    ['ftp://hs.example.com', null],
    ['https://hs.example.com?a=1', null],
    ['https://hs.example.com#x', null],
    ['https://u:p@hs.example.com', null],
    ['https://', null],
    ['https://hs.example.com:70000', null],
    ['https://hs.example.com:0', null],
    ['https://hs.example.com:abc', null],
    ['https://hs.example.com:', null],
    ['https://:443', null],
    ['https://---', null],
    ['https://hs.example.com/base path', null],
    [`https://${'a'.repeat(520)}`, null],
  ])('normalizes %j', (input, expected) => {
    expect(normalizeControlUrl(input)).toBe(expected);
  });

  it('checks the normalized length, including an inferred scheme', () => {
    const longest = 'https://' + 'a'.repeat(CONTROL_URL_MAX - 8);
    expect(normalizeControlUrl(longest)).toBe(longest);
    expect(normalizeControlUrl(longest + 'a')).toBeNull();
    expect(normalizeControlUrl(longest.slice(8) + '///')).toBe(longest);
  });
});
