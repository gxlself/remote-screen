export function installCommand(
  os: 'macos' | 'windows',
  baseUrl: string | null,
): string | null {
  if (baseUrl === null) {
    return null;
  }
  return os === 'macos'
    ? `curl -fsSL ${baseUrl}/host.sh | sh`
    : `powershell -c "irm ${baseUrl}/host.ps1 | iex"`;
}
