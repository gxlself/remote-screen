export type IncomingPairLink = { text: string } | { uri: string };

const MAX_LINK_LENGTH = 8192;
const PAIRFILE_LINK = /^rsp:\/\/pairfile\?/i;
const FILE_LINK = /^file:\/\//i;
const CONTENT_LINK = /^content:\/\//i;
const encoder = new TextEncoder();

export function isContentUrl(url: string): boolean {
  return CONTENT_LINK.test(url);
}

function filePath(url: string): string {
  const suffix = url.search(/[?#]/);
  const path = suffix === -1 ? url : url.slice(0, suffix);
  try {
    return decodeURIComponent(path);
  } catch {
    return path;
  }
}

export function classifyPairLink(url: string): IncomingPairLink | null {
  if (encoder.encode(url).length > MAX_LINK_LENGTH) {
    return null;
  }
  if (PAIRFILE_LINK.test(url)) {
    return { text: url };
  }
  if (!FILE_LINK.test(url)) {
    return null;
  }
  return filePath(url).toLowerCase().endsWith('.rspair') ? { uri: url } : null;
}
