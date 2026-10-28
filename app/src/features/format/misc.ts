const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;

export function errorCodeLabel(code: number): string {
  return `RS-E${code}`;
}

export function formatBytes(bytes: number): string {
  const sign = bytes < 0 ? '-' : '';
  let value = Math.abs(bytes);
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  const rounded = value.toFixed(1).replace(/\.0$/, '');
  return `${sign}${rounded} ${BYTE_UNITS[unitIndex]}`;
}
