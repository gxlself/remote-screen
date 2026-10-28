import { act, cleanup } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession, pairedHosts } from '../../native/rscore/fixtures';
import type { CoreEvent } from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import HistoryRecorder from './HistoryRecorder';
import {
  ADAPTER_LIVE_KEY,
  HISTORY_KEY,
  LIVE_SNAPSHOT_KEY,
  appendHistory,
  readHistory,
} from './history';
import { resetSshStore, sshStore } from './sshStore';

function sessionEvent(
  phase: Extract<CoreEvent, { type: 'session.state' }>['phase'],
): Extract<CoreEvent, { type: 'session.state' }> {
  return {
    type: 'session.state',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    phase,
    path: 'tailnetDirect',
    derpRegion: 'sea',
  };
}

function sshEvent(
  state: Extract<CoreEvent, { type: 'ssh.state' }>['state'],
): Extract<CoreEvent, { type: 'ssh.state' }> {
  return {
    type: 'ssh.state',
    sshId: 'ssh-1',
    serverId: 'gpu-train-02',
    state,
  };
}

beforeEach(() => {
  jest.useFakeTimers();
  resetSshStore();
});

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
});

async function emit(
  core: ReturnType<typeof createFakeCore>,
  event: CoreEvent,
): Promise<void> {
  await act(async () => {
    core.emit(event);
    await jest.advanceTimersByTimeAsync(1);
  });
}

it('writes a remote history entry after streaming and user close', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, { kv });

  await emit(rendered.core, sessionEvent('streaming'));
  await emit(rendered.core, {
    type: 'session.error',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    reason: 'userClosed',
  });

  expect(readHistory(kv)).toHaveLength(1);
  expect(readHistory(kv)[0]).toMatchObject({
    kind: 'remote',
    name: 'Studio Mac mini',
    targetId: liveSession.hostId,
    endReason: 'userClosed',
    path: 'tailnetDirect',
    derpRegion: 'sea',
  });
});

it('does not write remote history before streaming', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, {
    kv,
    core: createFakeCore({ sessions: [] }),
  });

  await emit(rendered.core, sessionEvent('connecting'));
  await emit(rendered.core, {
    type: 'session.error',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    reason: 'userClosed',
  });

  expect(readHistory(kv)).toEqual([]);
});

it('writes remote history for a background expiry', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, { kv });

  await emit(rendered.core, sessionEvent('streaming'));
  await emit(rendered.core, {
    type: 'session.error',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    reason: 'background',
  });

  expect(readHistory(kv)).toHaveLength(1);
  expect(readHistory(kv)[0]).toMatchObject({
    kind: 'remote',
    targetId: liveSession.hostId,
    endReason: 'background',
  });
});

it('records and removes SSH sessions that reached open', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, { kv });

  await emit(rendered.core, sshEvent('open'));
  expect(sshStore.getState()['ssh-1']?.state).toBe('open');

  await emit(rendered.core, sshEvent('closed'));
  expect(sshStore.getState()['ssh-1']).toBeUndefined();
  expect(readHistory(kv)[0]).toMatchObject({
    kind: 'ssh',
    targetId: 'gpu-train-02',
    name: 'gpu-train-02',
    endReason: 'closed',
  });
});

it('keeps the live snapshot while streaming and clears it afterwards', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, { kv });

  await emit(rendered.core, sessionEvent('streaming'));
  const snapshot = kv.getJSON<unknown[]>(LIVE_SNAPSHOT_KEY);
  expect(snapshot).toEqual([
    expect.objectContaining({
      id: `remote:${liveSession.id}`,
      kind: 'remote',
      name: 'Studio Mac mini',
    }),
  ]);

  await emit(rendered.core, {
    type: 'session.error',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    reason: 'userClosed',
  });
  expect(kv.getString(LIVE_SNAPSHOT_KEY)).toBeUndefined();
});

it('clears the SSH live snapshot when a terminal closes', async () => {
  const kv = new MemoryKv();
  const rendered = await renderWithProviders(<HistoryRecorder />, {
    kv,
    core: createFakeCore({ sessions: [] }),
  });

  await emit(rendered.core, sshEvent('open'));
  expect(kv.getJSON<unknown[]>(LIVE_SNAPSHOT_KEY)).toEqual([
    expect.objectContaining({
      id: 'ssh:ssh-1',
      kind: 'ssh',
      name: 'gpu-train-02',
    }),
  ]);

  await emit(rendered.core, sshEvent('failed'));
  expect(kv.getString(LIVE_SNAPSHOT_KEY)).toBeUndefined();
});

it('recovers a pre-existing live snapshot when mounted', async () => {
  const kv = new MemoryKv();
  kv.setJSON(LIVE_SNAPSHOT_KEY, [
    {
      id: 'leftover',
      kind: 'remote',
      targetId: liveSession.hostId,
      name: 'Studio Mac mini',
      startedAtUs: 1,
    },
  ]);

  await renderWithProviders(<HistoryRecorder />, {
    kv,
    core: createFakeCore({ sessions: [] }),
  });

  expect(readHistory(kv)).toEqual([
    expect.objectContaining({
      id: 'leftover',
      unclean: true,
    }),
  ]);
  expect(kv.getString(LIVE_SNAPSHOT_KEY)).toBeUndefined();
});

it('keeps an already recorded history entry when recovering', async () => {
  const kv = new MemoryKv();
  appendHistory(kv, {
    id: 'remote:1',
    kind: 'remote',
    targetId: 'host',
    name: 'Host',
    startedAtUs: 1,
    endedAtUs: 2,
    durationS: 0,
  });
  kv.setJSON(HISTORY_KEY, [...readHistory(kv)]);
  kv.setJSON(LIVE_SNAPSHOT_KEY, [
    {
      id: 'remote:1',
      kind: 'remote',
      targetId: 'host',
      name: 'Host',
      startedAtUs: 1,
    },
  ]);

  await renderWithProviders(<HistoryRecorder />, {
    kv,
    core: createFakeCore({ sessions: [] }),
  });

  expect(readHistory(kv)).toHaveLength(1);
  expect(readHistory(kv)[0].unclean).toBeUndefined();
});

it('names remote history from the local computer list without a Hub (H05)', async () => {
  const kv = new MemoryKv();
  const core = createFakeCore({
    pairedHosts: pairedHosts.map(host =>
      host.hostId === liveSession.hostId ? { ...host, name: 'Desk Mac' } : host,
    ),
  });
  const rendered = await renderWithProviders(<HistoryRecorder />, {
    kv,
    core,
  });
  await act(async () => {
    await jest.advanceTimersByTimeAsync(50);
  });

  await emit(rendered.core, sessionEvent('streaming'));
  await emit(rendered.core, {
    type: 'session.error',
    sessionId: liveSession.id,
    hostId: liveSession.hostId,
    reason: 'userClosed',
  });

  expect(readHistory(kv)[0]).toMatchObject({
    kind: 'remote',
    targetId: liveSession.hostId,
    name: 'Desk Mac',
  });
});

it('recovers adapter sessions left running by a killed app (H06)', async () => {
  const kv = new MemoryKv();
  kv.setJSON(ADAPTER_LIVE_KEY, [
    {
      id: 'adapter:9',
      kind: 'adapter',
      targetId: 'manual:nas',
      name: 'NAS Mac',
      startedAtUs: 1,
      protocol: 'vnc',
    },
  ]);

  await renderWithProviders(<HistoryRecorder />, {
    kv,
    core: createFakeCore({ sessions: [] }),
  });

  expect(readHistory(kv)).toEqual([
    expect.objectContaining({
      id: 'adapter:9',
      kind: 'adapter',
      unclean: true,
    }),
  ]);
  expect(kv.getString(ADAPTER_LIVE_KEY)).toBeUndefined();
});
