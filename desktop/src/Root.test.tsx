import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode } from 'react';
import { describe, expect, it } from 'vitest';

import { createRsCore } from '@app/native/rscore/createRsCore';

import { desktopBindings } from './core/bindings';
import { desktopApi } from './core/desktopApi';
import { FAKE_INFO, fakeTransport, type FakeOptions, type FakeTransport } from './core/fake';
import { Root } from './Root';

const PAGES = 'Remote Screen';

function renderRoot(opts?: FakeOptions): FakeTransport {
  const t = fakeTransport(opts);
  render(
    <StrictMode>
      <Root core={createRsCore(desktopBindings(t))} desktop={desktopApi(t)} />
    </StrictMode>,
  );
  return t;
}

function rscoreCalls(t: FakeTransport): Array<{ name: string; args: unknown[] }> {
  return t.calls
    .filter((c) => c.cmd === 'rscore_call')
    .map((c) => c.args as { name: string; args: unknown[] });
}

// Pages  loads the computer list as soon as it mounts; the startup checks skip those.
const PAGE_CALLS = new Set(['hostsPaired']);

function startupCalls(t: FakeTransport): Array<{ name: string; args: unknown[] }> {
  return rscoreCalls(t).filter((c) => !PAGE_CALLS.has(c.name));
}

function count(t: FakeTransport, name: string): number {
  return rscoreCalls(t).filter((c) => c.name === name).length;
}

describe('Root', () => {
  it('Root.registers', async () => {
    const t = renderRoot();

    expect(await screen.findByText(PAGES)).toBeInTheDocument();
    expect(t.calls[0]?.cmd).toBe('desktop_info');
    expect(startupCalls(t).map((c) => c.name)).toEqual(['coreInit', 'identityEnsure']);
    const calls = startupCalls(t);
    expect(calls[1]).toEqual({ name: 'identityEnsure', args: [FAKE_INFO.deviceName] });
    const init = JSON.parse(String(calls[0]?.args[0])) as Record<string, unknown>;
    expect(init).toEqual({
      dataDir: FAKE_INFO.dataDir,
      appVersion: FAKE_INFO.version,
      platform: 'macos',
    });
  });

  it('Root.other_failure_redacts', async () => {
    const user = userEvent.setup();
    const name = FAKE_INFO.deviceName;
    let fail = true;
    const t = renderRoot({
      rscore: {
        identityEnsure: () => {
          if (fail) {
            throw {
              tag: 'Internal',
              inner: {
                message: `write /Users/demo/Library/core/identity.json for ${name} via 100.64.0.1:7300 failed`,
              },
            };
          }
          return JSON.stringify({ deviceId: 'd5k2m8q4x7v1tb3n', fp: 'x' });
        },
      },
    });

    expect(await screen.findByText('注册失败')).toBeInTheDocument();
    expect(screen.getByText(`write … for ${name} via … failed`)).toBeInTheDocument();
    expect(screen.queryByText(PAGES)).toBeNull();

    fail = false;
    await user.click(screen.getByRole('button', { name: '重试' }));
    await waitFor(() => expect(count(t, 'identityEnsure')).toBe(2));
    expect(await screen.findByText(PAGES)).toBeInTheDocument();
    expect(count(t, 'coreInit')).toBe(1);
  });

  it('Root.info_failure_retries_from_start', async () => {
    const user = userEvent.setup();
    const t = fakeTransport();
    const desktop = desktopApi(t);
    let infoCalls = 0;
    const flaky = {
      ...desktop,
      info: () => {
        infoCalls += 1;
        return infoCalls === 1
          ? Promise.reject(new Error('shell not ready at /Users/demo/x/y'))
          : desktop.info();
      },
    };
    render(<Root core={createRsCore(desktopBindings(t))} desktop={flaky} />);

    expect(await screen.findByText('注册失败')).toBeInTheDocument();
    expect(screen.getByText('shell not ready at …')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '重试' }));

    expect(await screen.findByText(PAGES)).toBeInTheDocument();
    expect(infoCalls).toBe(2);
    expect(count(t, 'coreInit')).toBe(1);
  });
});
