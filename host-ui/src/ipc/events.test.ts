import { describe, expect, it } from 'vitest';

import { parseEvent } from './events';

describe('parseEvent', () => {
  it('parses the pairing approval payload produced by pairing.rs', () => {
    const payload = {
      request_id: 'r-1',
      kind: 'pairing',
      device_name: '小明的 iPhone',
      platform: 'ios',
      whois_login: 'alice@',
      device_fp: 'A'.repeat(43),
    };

    expect(parseEvent('event.approvalRequested', payload)).toEqual({
      request_id: 'r-1',
      kind: 'pairing',
      device_name: '小明的 iPhone',
      device_id: '',
      platform: 'ios',
      whois_login: 'alice@',
      path: null,
      reasons: [],
      timeout_s: 30,
      permission_choice: false,
      device_fp: 'A'.repeat(43),
    });
  });

  it('defaults missing optional approval fields', () => {
    const parsed = parseEvent('event.approvalRequested', {
      request_id: 'r-2',
      kind: 'connection',
    });

    expect(parsed?.whois_login).toBeNull();
    expect(parsed?.platform).toBe('unknown');
    expect(parsed?.device_name).toBe('');
    expect(parsed?.device_id).toBe('');
    expect(parsed?.reasons).toEqual([]);
    expect(parsed?.permission_choice).toBe(false);
    expect(parsed?.device_fp).toBeNull();
    expect(
      parseEvent('event.approvalRequested', {
        request_id: 'r-2',
        kind: 'connection',
        reasons: ['untrusted-path', 1],
      })?.reasons,
    ).toEqual([]);
  });

  it('rejects an approval request with a non-string kind', () => {
    expect(parseEvent('event.approvalRequested', { request_id: 'r-3', kind: 1 })).toBeNull();
  });

  it('parses a pairing result and requires a numeric status', () => {
    const payload = {
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
    };

    expect(parseEvent('event.pairingResult', payload)).toEqual(payload);
    expect(parseEvent('event.pairingResult', { host_id: 'h' })).toBeNull();
  });

  it('parses pairing result method, device name and account fields', () => {
    const parsed = parseEvent('event.pairingResult', {
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
      method: 'password',
      device_name: '小明的 iPhone',
      account: 'alice',
    });

    expect(parsed).toEqual({
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
      method: 'password',
      device_name: '小明的 iPhone',
      account: 'alice',
    });
  });

  it('normalizes an unknown pairing method to no method', () => {
    const parsed = parseEvent('event.pairingResult', {
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
      method: 'weird',
      device_name: '手机',
      account: 'someone',
    });

    expect(parsed).toEqual({
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
      device_name: '手机',
      account: 'someone',
    });
    expect(parsed?.method).toBeUndefined();
  });

  it('keeps parsing legacy pairing results without the new fields', () => {
    const legacy = {
      status: 1,
      host_id: 'h',
      host_name: 'n',
      attempts_left: 0,
      device_fp: 'x',
    };

    expect(parseEvent('event.pairingResult', legacy)).toEqual(legacy);
  });

  it('normalizes a null sessionsChanged payload to an empty object', () => {
    expect(parseEvent('event.sessionsChanged', null)).toEqual({});
  });

  it('validates approval close payloads', () => {
    expect(
      parseEvent('event.approvalClosed', {
        request_id: 'r-4',
        kind: 'takeover',
        outcome: 'allowed',
      }),
    ).toEqual({ request_id: 'r-4', kind: 'takeover', outcome: 'allowed' });
    expect(
      parseEvent('event.approvalClosed', {
        request_id: 'r-4',
        kind: 'takeover',
        outcome: 'unknown',
      }),
    ).toBeNull();
  });

  it('copies object events and ignores non-object payloads', () => {
    const payload = { state: 'downloading' };
    expect(parseEvent('event.updateState', payload)).toEqual(payload);
    expect(parseEvent('event.updateState', { state: 1 })).toEqual({});
    expect(parseEvent('event.permissionsChanged', [])).toEqual({});
    expect(parseEvent('event.handoff', null)).toEqual({});
  });
});
