/** Launch argument passed by SceneDelegate; host validation stays in the C ABI. */
export function initialConnection(autoConnect?: string) {
  const match = /^(?:\[([^\]]+)\]|([^:\s]+)):(\d{1,5})$/.exec(autoConnect ?? '');
  const port = match ? Number(match[3]) : 0;
  if (match && port > 0 && port <= 65535) {
    return { host: match[1] ?? match[2], port, enabled: true };
  }
  return { host: '127.0.0.1', port: 7400, enabled: false };
}
