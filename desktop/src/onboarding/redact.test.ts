import { describe, expect, it } from 'vitest';

import { redactMessage } from './redact';

describe('redactMessage', () => {
  it('hides absolute paths and addresses but keeps the device name', () => {
    const name = '我的 MacBook Pro';
    expect(
      redactMessage(`open /Users/demo/core/a.db for ${name}: dial 192.168.31.5:7400 refused`, name),
    ).toBe(`open … for ${name}: dial … refused`);
    expect(redactMessage('read C:\\Users\\demo\\core\\a.db failed', '')).toBe('read … failed');
    expect(redactMessage('dial [fd7a:115c::5]:7400 timed out', '')).toBe('dial … timed out');
    expect(redactMessage('service http://example.com:7300/v1/devices said no', '')).toBe(
      'service http://example.com:7300/v1/devices said no',
    );
    expect(redactMessage('device /dev/name kept', '/dev/name')).toBe('device /dev/name kept');
  });
});
