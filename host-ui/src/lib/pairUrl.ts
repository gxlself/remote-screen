import { decodeBase64Url } from './format';

export interface PairUrl {
  v: 1;
  hostId: string;
  fp: string;
  code: string;
  name: string;
  addrs: string[];
}

function percentDecode(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

function required(params: Map<string, string>, key: string): string | null {
  return params.get(key) ?? null;
}

export function parsePairUrl(url: string): PairUrl | null {
  if (!url.startsWith('rsp://pair?') || url.includes('#')) {
    return null;
  }

  const query = url.slice('rsp://pair?'.length);
  const params = new Map<string, string>();
  for (const parameter of query.split('&')) {
    const equals = parameter.indexOf('=');
    if (equals < 0) {
      return null;
    }
    const key = percentDecode(parameter.slice(0, equals));
    const value = percentDecode(parameter.slice(equals + 1));
    if (key === null || value === null || params.has(key)) {
      return null;
    }
    params.set(key, value);
  }

  const version = required(params, 'v');
  const hostId = required(params, 'h');
  const fp = required(params, 'fp');
  const code = required(params, 'c');
  const name = required(params, 'n');
  const addressList = required(params, 'a');
  if (
    version !== '1' ||
    hostId === null ||
    fp === null ||
    decodeBase64Url(fp, false)?.length !== 32 ||
    code === null ||
    code.length !== 6 ||
    !/^[0-9]{6}$/.test(code) ||
    name === null ||
    [...name].length > 64 ||
    addressList === null
  ) {
    return null;
  }

  const addrs = addressList.split(',');
  if (addrs.length < 1 || addrs.length > 8 || addrs.some((address) => address.length === 0)) {
    return null;
  }

  return { v: 1, hostId, fp, code, name, addrs };
}

export function hostOfAddr(addr: string): string {
  if (addr.startsWith('[')) {
    const closingBracket = addr.indexOf(']');
    if (closingBracket > 0 && addr[closingBracket + 1] === ':') {
      return addr.slice(1, closingBracket);
    }
    return addr;
  }

  const colon = addr.indexOf(':');
  return colon > 0 && colon === addr.lastIndexOf(':') ? addr.slice(0, colon) : addr;
}
