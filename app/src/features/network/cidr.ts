const IPV4_OCTET_MAX = 255;
const IPV4_BITS = 32;
const TAILNET_PREFIX = '100.64.0.0/10';

function parseIpv4Octets(value: string): number[] | null {
  const parts = value.split('.');
  if (parts.length !== 4) {
    return null;
  }

  const octets: number[] = [];
  for (const part of parts) {
    if (!/^(0|[1-9]\d{0,2})$/.test(part)) {
      return null;
    }
    const octet = Number(part);
    if (octet > IPV4_OCTET_MAX) {
      return null;
    }
    octets.push(octet);
  }
  return octets;
}

function octetsToNumber(octets: number[]): number {
  return (
    octets[0] * 16_777_216 + octets[1] * 65_536 + octets[2] * 2 ** 8 + octets[3]
  );
}

/** 将严格点分十进制的 IPv4 地址转为无符号 32 位整数。 */
export function parseIpv4(value: string): number | null {
  const octets = parseIpv4Octets(value);
  return octets === null ? null : octetsToNumber(octets);
}

function parseCidr(
  value: string,
): { network: number; blockSize: number } | null {
  const slash = value.indexOf('/');
  if (slash === -1 || value.indexOf('/', slash + 1) !== -1) {
    return null;
  }

  const address = parseIpv4(value.slice(0, slash));
  const prefixText = value.slice(slash + 1);
  if (
    address === null ||
    !/^(0|[1-9]\d?)$/.test(prefixText) ||
    Number(prefixText) > IPV4_BITS
  ) {
    return null;
  }

  const prefix = Number(prefixText);
  const blockSize = 2 ** (IPV4_BITS - prefix);
  return { network: address - (address % blockSize), blockSize };
}

export function isValidCidr(value: string): boolean {
  return parseCidr(value) !== null;
}

export function ipInCidr(ip: string, cidr: string): boolean {
  const address = parseIpv4(ip);
  const parsed = parseCidr(cidr);
  if (address === null || parsed === null) {
    return false;
  }
  return address - (address % parsed.blockSize) === parsed.network;
}

export function isTailnetAddr(ip: string): boolean {
  return ipInCidr(ip, TAILNET_PREFIX);
}
