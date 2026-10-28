import { initialConnection } from './launchArguments';

describe('unattended autoConnect launch argument', () => {
  it.each([
    ['127.0.0.1:7400', '127.0.0.1', 7400],
    ['[::1]:65535', '::1', 65535],
    ['100.64.0.1:1', '100.64.0.1', 1],
  ])('accepts %s', (argument, host, port) => {
    expect(initialConnection(String(argument))).toEqual({ host, port, enabled: true });
  });

  it.each([undefined, '', '127.0.0.1', '::1:7400', '127.0.0.1:0', '127.0.0.1:65536', 'host:abc'])
    ('keeps the manual connection disabled for %s', argument => {
      expect(initialConnection(argument).enabled).toBe(false);
    });
});
