import { installCommand } from './installCommand';

describe('installCommand', () => {
  it('installs on macOS with the shell script', () => {
    expect(installCommand('macos', 'https://dl.example.com/rs')).toBe(
      'curl -fsSL https://dl.example.com/rs/host.sh | sh',
    );
  });

  it('installs on Windows with the PowerShell script', () => {
    expect(installCommand('windows', 'https://dl.example.com/rs')).toBe(
      'powershell -c "irm https://dl.example.com/rs/host.ps1 | iex"',
    );
  });

  it('has no command when Hub did not configure an install URL', () => {
    expect(installCommand('macos', null)).toBeNull();
    expect(installCommand('windows', null)).toBeNull();
  });
});
