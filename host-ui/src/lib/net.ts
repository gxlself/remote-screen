export function isIPv4(value: string): boolean {
  const parts = value.split('.');
  return (
    parts.length === 4 &&
    parts.every((part) => {
      const match = /^(0|[1-9][0-9]{0,2})$/.exec(part);
      return match !== null && match[0] === part && Number(part) <= 255;
    })
  );
}

function isLooseIPv6(value: string): boolean {
  const match = /^[0-9a-fA-F:]+$/.exec(value);
  if (match === null || match[0] !== value) {
    return false;
  }

  const doubleColon = value.indexOf('::');
  if (doubleColon >= 0 && value.indexOf('::', doubleColon + 2) >= 0) {
    return false;
  }
  if (value.includes(':::')) {
    return false;
  }
  if (
    (value.startsWith(':') && !value.startsWith('::')) ||
    (value.endsWith(':') && !value.endsWith('::'))
  ) {
    return false;
  }

  const groups = value.split(':').filter((group) => group.length > 0);
  return groups.length <= 8;
}

export function isValidCidr(value: string): boolean {
  const cidr = value.trim();
  const slash = cidr.indexOf('/');
  if (slash <= 0 || slash !== cidr.lastIndexOf('/')) {
    return false;
  }

  const address = cidr.slice(0, slash);
  const prefixText = cidr.slice(slash + 1);
  const prefixMatch = /^[0-9]+$/.exec(prefixText);
  if (prefixMatch === null || prefixMatch[0] !== prefixText) {
    return false;
  }

  const prefix = Number(prefixText);
  if (address.includes(':')) {
    return isLooseIPv6(address) && prefix >= 0 && prefix <= 128;
  }
  return isIPv4(address) && prefix >= 0 && prefix <= 32;
}

export function firstIPv4(addresses: unknown): string | null {
  if (!Array.isArray(addresses)) {
    return null;
  }
  for (const address of addresses as unknown[]) {
    if (typeof address === 'string' && isIPv4(address)) {
      return address;
    }
  }
  return null;
}
