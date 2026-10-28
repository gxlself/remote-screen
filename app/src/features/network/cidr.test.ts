import { ipInCidr, isTailnetAddr, isValidCidr, parseIpv4 } from './cidr';

describe('parseIpv4', () => {
  it.each([
    ['0.0.0.0', 0],
    ['10.99.4.27', 0x0a63041b],
    ['255.255.255.255', 0xffffffff],
  ])('parses %s as %s', (value, expected) => {
    expect(parseIpv4(value)).toBe(expected);
  });

  it.each([
    '',
    '10.99.4',
    '10.99.4.27.1',
    '256.1.1.1',
    '10.99.4.-1',
    '10.99.4.01',
    '10.99.4.a',
    ' 10.99.4.27',
    '10.99.4.27 ',
  ])('rejects invalid IPv4 %p', value => {
    expect(parseIpv4(value)).toBeNull();
  });
});

describe('ipInCidr', () => {
  it.each([
    ['10.99.4.27', '10.99.0.0/16', true],
    ['10.98.0.1', '10.99.0.0/16', false],
    ['192.168.31.255', '192.168.31.0/24', true],
    ['192.168.32.1', '192.168.31.0/24', false],
    ['8.8.8.8', '0.0.0.0/0', true],
  ])('%s in %s is %s', (ip, cidr, expected) => {
    expect(ipInCidr(ip, cidr)).toBe(expected);
  });

  it.each([
    ['10.99.4.27', '10.99.0.0'],
    ['10.99.4.27', '10.99.0.0/33'],
    ['10.99.4.27', '10.99.0.0/'],
    ['10.99.4.27', '10.99.0.0/16/1'],
    ['not-an-ip', '10.99.0.0/16'],
    ['10.99.4.27', 'not-a-cidr'],
  ])('returns false for ip %p and cidr %p', (ip, cidr) => {
    expect(ipInCidr(ip, cidr)).toBe(false);
  });
});

describe('isTailnetAddr', () => {
  it.each([
    ['100.64.0.21', true],
    ['100.64.0.0', true],
    ['100.127.255.255', true],
    ['100.128.0.1', false],
    ['100.63.255.255', false],
    ['not-an-ip', false],
  ])('%s is tailnet: %s', (value, expected) => {
    expect(isTailnetAddr(value)).toBe(expected);
  });
});

describe('isValidCidr', () => {
  it.each([
    ['10.99.0.0/16', true],
    ['0.0.0.0/0', true],
    ['255.255.255.255/32', true],
    ['10.99.0.0', false],
    ['10.99.0.0/33', false],
    ['10.99.0.0/-1', false],
    ['10.99.0.0/016', false],
    ['10.99.0.0/16/2', false],
    ['10.99.0.256/16', false],
  ])('%s valid: %s', (value, expected) => {
    expect(isValidCidr(value)).toBe(expected);
  });
});
