import NativeRsDevLog from '../specs/NativeRsDevLog';

const tagPattern = /^[A-Z0-9-]{1,32}$/;

export function harnessLog(
  tag: string,
  payload: Record<string, unknown>,
): void {
  const normalizedTag = tagPattern.test(tag) ? tag : 'INVALID';
  let encodedPayload: string;
  try {
    encodedPayload = JSON.stringify(payload);
  } catch {
    encodedPayload = JSON.stringify({ error: 'unserializable' });
  }
  const line = `RS-${normalizedTag} ${encodedPayload}`;
  NativeRsDevLog?.log?.(line);
  if (__DEV__) {
    console.log(line);
  }
}
