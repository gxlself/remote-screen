const IPV6 = /\[[0-9A-Fa-f.]*:[0-9A-Fa-f:.]*\](?::\d+)?/g;
const IPV4 = /\b\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?\b/g;
const WINDOWS_PATH = /\b[A-Za-z]:\\[^\s'"]*/g;
// "/a/…" after the start, whitespace or an opening quote; URL paths ("http://x/y") stay.
const POSIX_PATH = /(^|[\s'"(=,])\/(?:[^\s/'"]+\/)+[^\s'",)]*/g;

function redactPart(part: string): string {
  return part
    .replace(IPV6, '…')
    .replace(IPV4, '…')
    .replace(WINDOWS_PATH, '…')
    .replace(POSIX_PATH, '$1…');
}

/** Replaces absolute paths and IP addresses in an error message with "…", keeping the device name. */
export function redactMessage(message: string, deviceName: string): string {
  if (deviceName === '') {
    return redactPart(message);
  }
  return message.split(deviceName).map(redactPart).join(deviceName);
}
