import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { describe, expect, it } from 'vitest';

import {
  METHOD_NAMES,
  type DeepPartial,
  type EmptyResult,
  type HostSettings,
  type MethodName,
  type Params,
  type Result,
} from './types';

function fixturesDirectory(): string {
  const fixtureUrl = new URL('../../../crates/host-core/tests/ipc/', import.meta.url);
  if (fixtureUrl.protocol === 'file:') {
    return fileURLToPath(fixtureUrl);
  }

  const testFileUrl = pathToFileURL(`${process.cwd()}/src/ipc/types.contract.test.ts`);
  return fileURLToPath(new URL('../../../crates/host-core/tests/ipc/', testFileUrl));
}

const fixturesDir = fixturesDirectory();

function readFixture(method: MethodName, kind: 'request' | 'response'): unknown {
  return JSON.parse(readFileSync(`${fixturesDir}${method}.${kind}.json`, 'utf8')) as unknown;
}

function responseFixture<M extends MethodName>(method: M): Result<M> {
  const fixture = readFixture(method, 'response') as { result: Result<M> };
  return fixture.result;
}

function paramsFixture<M extends MethodName>(method: M): Params<M> {
  const fixture = readFixture(method, 'request') as { params?: unknown };
  return (fixture.params ?? null) as Params<M>;
}

describe('IPC fixture contract', () => {
  it('has exactly one request/response pair for every method', () => {
    const fixtureMethods = new Set<string>();
    for (const file of readdirSync(fixturesDir)) {
      const match = /^(.+)\.(request|response)\.json$/.exec(file);
      if (match?.[1]) {
        fixtureMethods.add(match[1]);
      }
    }

    expect([...fixtureMethods].sort()).toEqual([...METHOD_NAMES].sort());
  });

  it('matches status.get', () => {
    const result = {
      online: true,
      tailnet_addresses: ['100.64.0.2'],
      version: '0.1.0',
      permissions: { screen_recording: true },
      listening: true,
      sessions: [],
    } satisfies Result<'status.get'>;

    expect(result).toEqual(responseFixture('status.get'));
  });

  it('matches sessions.list', () => {
    const result = {
      sessions: [
        {
          session_id: 's-1',
          device_id: 'd-1',
          device_name: 'test phone',
          role: 'viewer',
          connected_at_us: 0,
          recording: false,
        },
      ],
    } satisfies Result<'sessions.list'>;

    expect(result).toEqual(responseFixture('sessions.list'));
  });

  it('matches sessions.disconnect', () => {
    const params = { session_id: 's-example' } satisfies Params<'sessions.disconnect'>;
    const result = {} satisfies Result<'sessions.disconnect'>;

    expect(params).toEqual(paramsFixture('sessions.disconnect'));
    expect(result).toEqual(responseFixture('sessions.disconnect'));
  });

  it('matches sessions.setViewOnly', () => {
    const params = { session_id: 's-example' } satisfies Params<'sessions.setViewOnly'>;
    const result = {} satisfies Result<'sessions.setViewOnly'>;

    expect(params).toEqual(paramsFixture('sessions.setViewOnly'));
    expect(result).toEqual(responseFixture('sessions.setViewOnly'));
  });

  it('matches pairing.start', () => {
    const result = {
      qr_url: 'rs://pair/example',
      code: '123456',
      expires_at: 1800000000000000,
    } satisfies Result<'pairing.start'>;

    expect(result).toEqual(responseFixture('pairing.start'));
  });

  it('matches pairing.cancel', () => {
    const params = {} satisfies Params<'pairing.cancel'>;
    const result = {} satisfies Result<'pairing.cancel'>;

    expect(params).toEqual(paramsFixture('pairing.cancel'));
    expect(result).toEqual(responseFixture('pairing.cancel'));
  });

  it('matches pairing.exportFile', () => {
    const params = {
      permission: 'control',
      label: 'IT 分发 10 月',
    } satisfies Params<'pairing.exportFile'>;
    const result = {
      file_id: 'AAECAwQFBgcICQoLDA0ODw',
      path: '/Users/example/Downloads/Studio Mac mini-20261020-0258.rspair',
      file_name: 'Studio Mac mini-20261020-0258.rspair',
      expires_at: 1792551534000000,
    } satisfies Result<'pairing.exportFile'>;

    expect(params).toEqual(paramsFixture('pairing.exportFile'));
    expect(result).toEqual(responseFixture('pairing.exportFile'));
  });

  it('matches pairing.listFiles', () => {
    const params = null satisfies Params<'pairing.listFiles'>;
    const result = {
      files: [
        {
          file_id: 'AAECAwQFBgcICQoLDA0ODw',
          label: 'IT 分发 10 月',
          permission: 'control',
          created_at: 1792465134000000,
          expires_at: 1792551534000000,
          state: 'active',
          used_by_device_id: null,
          used_at: null,
        },
        {
          file_id: 'EBESExQVFhcYGRobHB0eHw',
          label: 'iPad 现场支持',
          permission: 'view_only',
          created_at: 1792378734000000,
          expires_at: 1792465134000000,
          state: 'used',
          used_by_device_id: 'AAECAwQFBgcICQoLDA0ODw',
          used_at: 1792400000000000,
        },
      ],
    } satisfies Result<'pairing.listFiles'>;

    expect(params).toEqual(paramsFixture('pairing.listFiles'));
    expect(result).toEqual(responseFixture('pairing.listFiles'));
  });

  it('matches pairing.revokeFile', () => {
    const params = {
      file_id: 'AAECAwQFBgcICQoLDA0ODw',
    } satisfies Params<'pairing.revokeFile'>;
    const result = {} satisfies Result<'pairing.revokeFile'>;

    expect(params).toEqual(paramsFixture('pairing.revokeFile'));
    expect(result).toEqual(responseFixture('pairing.revokeFile'));
  });

  it('matches devices.list', () => {
    const result = { devices: [] } satisfies Result<'devices.list'>;

    expect(result).toEqual(responseFixture('devices.list'));
  });

  it('matches devices.update', () => {
    const params = {
      device_id: 'd-example',
      permission: 'control',
      paths: [],
    } satisfies Params<'devices.update'>;
    const result = {} satisfies Result<'devices.update'>;

    expect(params).toEqual(paramsFixture('devices.update'));
    expect(result).toEqual(responseFixture('devices.update'));
  });

  it('matches devices.revoke', () => {
    const params = { device_id: 'd-example' } satisfies Params<'devices.revoke'>;
    const result = {} satisfies Result<'devices.revoke'>;

    expect(params).toEqual(paramsFixture('devices.revoke'));
    expect(result).toEqual(responseFixture('devices.revoke'));
  });

  it('matches settings.get', () => {
    // The host-core fixtures intentionally contain only the access section.
    const result = {
      access: { allow_remote_control: true },
    } satisfies DeepPartial<HostSettings>;

    expect(result).toEqual(responseFixture('settings.get'));
  });

  it('matches settings.set', () => {
    // Partial updates are represented by a deep partial of the full settings tree.
    const params = {
      access: { allow_remote_control: true },
    } satisfies DeepPartial<HostSettings>;
    const result = {
      access: { allow_remote_control: true },
    } satisfies DeepPartial<HostSettings>;

    expect(params).toEqual(paramsFixture('settings.set'));
    expect(result).toEqual(responseFixture('settings.set'));
  });

  it('matches network.listNics', () => {
    const result = { nics: [] } satisfies Result<'network.listNics'>;

    expect(result).toEqual(responseFixture('network.listNics'));
  });

  it('matches network.firewallStatus', () => {
    const result = {
      enabled: false,
      active: false,
      detail: {},
    } satisfies Result<'network.firewallStatus'>;

    expect(result).toEqual(responseFixture('network.firewallStatus'));
  });

  it('matches network.itNote', () => {
    const result = { text: '' } satisfies Result<'network.itNote'>;

    expect(result).toEqual(responseFixture('network.itNote'));
  });

  it('matches approval.pending', () => {
    const params = null satisfies Params<'approval.pending'>;
    const result = {
      requests: [
        {
          request_id: 'a-1',
          kind: 'connection',
          device_name: 'test phone',
          device_id: 'd-1',
          platform: 'ios',
          whois_login: null,
          path: 'lan',
          reasons: ['attended_mode'],
          timeout_s: 30,
          permission_choice: false,
          device_fp: null,
        },
      ],
    } satisfies Result<'approval.pending'>;

    expect(params).toEqual(paramsFixture('approval.pending'));
    expect(result).toEqual(responseFixture('approval.pending'));
  });

  it('matches approval.respond', () => {
    const params = {
      request_id: 'r-example',
      allow: true,
    } satisfies Params<'approval.respond'>;
    const result = {} satisfies Result<'approval.respond'>;

    expect(params).toEqual(paramsFixture('approval.respond'));
    expect(result).toEqual(responseFixture('approval.respond'));
  });

  it('matches update.status', () => {
    const result = {
      state: '',
      version: null,
      notes: null,
    } satisfies Result<'update.status'>;

    expect(result).toEqual(responseFixture('update.status'));
  });

  it('matches update.installNow', () => {
    const result = {} satisfies Result<'update.installNow'>;

    expect(result).toEqual(responseFixture('update.installNow'));
  });

  it('matches update.setChannel', () => {
    const params = { channel: 'stable' } satisfies Params<'update.setChannel'>;
    const result = {} satisfies Result<'update.setChannel'>;

    expect(params).toEqual(paramsFixture('update.setChannel'));
    expect(result).toEqual(responseFixture('update.setChannel'));
  });

  it('matches logs.export', () => {
    const params = { mask: true } satisfies Params<'logs.export'>;
    const result = { path: '/tmp/remote-host-logs.zip' } satisfies Result<'logs.export'>;

    expect(params).toEqual(paramsFixture('logs.export'));
    expect(result).toEqual(responseFixture('logs.export'));
  });

  it('matches audit.list', () => {
    const params = {} satisfies Params<'audit.list'>;
    const result = { entries: [] } satisfies Result<'audit.list'>;

    expect(params).toEqual(paramsFixture('audit.list'));
    expect(result).toEqual(responseFixture('audit.list'));
  });

  it('keeps EmptyResult assignable to empty fixtures', () => {
    const empty = {} satisfies EmptyResult;
    expect(empty).toEqual({});
  });
});
