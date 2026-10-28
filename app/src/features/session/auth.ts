export type SessionAuthSettings = {
  beforeSession: boolean;
  sensitiveHosts: string[];
};

export function needsSessionAuth(
  hostId: string,
  settings: SessionAuthSettings,
): boolean {
  return settings.beforeSession || settings.sensitiveHosts.includes(hostId);
}
