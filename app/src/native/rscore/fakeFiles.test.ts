import { createFakeCore } from './fakeCore';
import type { FakeCore } from './fakeCore';
import { RsCoreError } from './RsCoreApi';
import { computers, sshServers } from './fixtures';
import type { FileTransfer, FileTransferState } from './types';

const hostId = computers[0]?.hostId ?? '';
const serverId = sshServers[0]?.serverId ?? '';

function transfer(core: FakeCore, id: string): FileTransfer {
  const found = core.transfers.list().find(item => item.transferId === id);
  if (found === undefined) {
    throw new Error(`missing transfer ${id}`);
  }
  return found;
}

async function advanceToDone(): Promise<void> {
  for (let index = 0; index < 12; index += 1) {
    jest.advanceTimersByTime(100);
    await Promise.resolve();
  }
}

describe('FakeCore files', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.clearAllTimers();
    jest.useRealTimers();
  });

  it('lists the paired computer root, rejects bad paths and unpaired hosts', async () => {
    const core = createFakeCore();

    await expect(core.files.list(hostId, null)).resolves.toMatchObject({
      path: '/Users/demo',
      parent: null,
      netPath: 'tailnetDirect',
    });
    const root = await core.files.list(hostId, null);
    expect(root.entries.map(entry => entry.name)).toEqual(
      expect.arrayContaining(['Desktop', 'Documents', 'Downloads']),
    );
    expect(root.entries).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: 'file' }),
        expect.objectContaining({ kind: 'symlink' }),
        expect.objectContaining({ kind: 'other' }),
        expect.objectContaining({
          name: 'Q3-remote-screen-accessibility-review-notes-final-approved-2026-10-20.pdf',
        }),
      ]),
    );

    const missing = await core.files
      .list(hostId, '/no/such')
      .catch(error => error);
    expect(missing).toBeInstanceOf(RsCoreError);
    expect(missing).toMatchObject({
      kind: 'http',
      httpStatus: 404,
      message: 'files.notFound: /no/such',
    });
    const unpaired = await core.files
      .list('no-such-host', null)
      .catch(error => error);
    expect(unpaired).toMatchObject({ kind: 'unsupported' });
  });

  it('lists a server default root and reports unknown servers as 404', async () => {
    const core = createFakeCore();
    await expect(core.sftp.list(serverId, null)).resolves.toMatchObject({
      path: '/home/demo',
      parent: null,
      netPath: null,
      entries: expect.arrayContaining([
        expect.objectContaining({ name: 'logs', kind: 'dir' }),
        expect.objectContaining({ name: 'app.tar.gz', kind: 'file' }),
      ]),
    });
    await expect(core.sftp.list('missing-sftp', null)).rejects.toMatchObject({
      kind: 'http',
      httpStatus: 404,
      message: expect.stringContaining('sftp.serverNotFound'),
    });
  });

  it('advances a download through verifying and completion', async () => {
    const core = createFakeCore();
    const allStates: FileTransferState[] = [];
    core.events.subscribe(event => {
      if (event.type === 'file.transfer') {
        allStates.push(event.transfer.state);
      }
    });

    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    expect(transfer(core, id).state).toBe('queued');
    await advanceToDone();
    expect([...new Set(allStates)]).toEqual([
      'queued',
      'running',
      'verifying',
      'completed',
    ]);
    expect(transfer(core, id)).toMatchObject({
      state: 'completed',
      done: 6144,
      localPath: '/fake/files/notes-1020.md',
    });
  });

  it('pauses without advancing and resumes from the same done value', async () => {
    const core = createFakeCore();
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    jest.advanceTimersByTime(200);
    await Promise.resolve();
    core.transfers.pause(id);
    const paused = transfer(core, id);
    expect(paused.state).toBe('paused');
    const done = paused.done;
    jest.advanceTimersByTime(500);
    expect(transfer(core, id).done).toBe(done);
    await core.transfers.resume(id);
    expect(transfer(core, id).state).toBe('running');
    await advanceToDone();
    expect(transfer(core, id)).toMatchObject({
      state: 'completed',
      done: 6144,
    });
  });

  it('cancels, removes and emits removal events', async () => {
    const core = createFakeCore();
    const events: string[] = [];
    core.events.subscribe(event => {
      if (event.type === 'file.transferRemoved') {
        events.push(event.transferId);
      }
    });
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    core.transfers.cancel(id);
    expect(transfer(core, id).state).toBe('cancelled');
    core.transfers.remove(id);
    expect(core.transfers.list().some(item => item.transferId === id)).toBe(
      false,
    );
    expect(events).toEqual([id]);
  });

  it('fails and interrupts through test hooks', async () => {
    const core = createFakeCore();
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    core.failTransfer(id, 'sha256Mismatch');
    expect(transfer(core, id)).toMatchObject({
      state: 'failed',
      fail: 'sha256Mismatch',
    });
    await core.transfers.resume(id);
    core.interruptTransfer(id);
    expect(transfer(core, id).state).toBe('interrupted');
    await core.transfers.resume(id);
    await advanceToDone();
    expect(transfer(core, id).state).toBe('completed');
  });

  it('reports notFound for unknown ids and notResumable for completed ones', async () => {
    const core = createFakeCore();
    await expect(core.transfers.resume('missing')).rejects.toMatchObject({
      kind: 'http',
      httpStatus: 404,
      message: 'transfers.notFound: missing',
    });
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    await advanceToDone();
    await expect(core.transfers.resume(id)).rejects.toMatchObject({
      kind: 'http',
      httpStatus: 409,
      message: `transfers.notResumable: ${id}`,
    });
  });

  it('completes an SFTP upload with a saved name', async () => {
    const core = createFakeCore();
    const id = await core.sftp.upload(
      serverId,
      '/tmp/report.pdf',
      '/home/demo',
    );
    await advanceToDone();
    expect(transfer(core, id)).toMatchObject({
      direction: 'upload',
      savedName: 'report.pdf',
      state: 'completed',
    });
  });

  it('honours a custom tick interval and keeps the queued default', async () => {
    const core = createFakeCore({}, { tickMs: 25 });
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    jest.advanceTimersByTime(0);
    await Promise.resolve();
    expect(transfer(core, id).state).toBe('running');
    jest.advanceTimersByTime(25);
    await Promise.resolve();
    expect(transfer(core, id).done).toBeGreaterThan(0);
    expect(transfer(core, id)).toMatchObject({
      remotePath: '/Users/demo/notes-1020.md',
    });
  });

  it('applies failNext to file and transfer methods', async () => {
    const core = createFakeCore();
    core.failNext('files.list', new Error('list failed'));
    await expect(core.files.list(hostId, null)).rejects.toThrow('list failed');
    const id = await core.files.download(hostId, '/Users/demo/notes-1020.md');
    core.failNext('transfers.pause', new Error('pause failed'));
    expect(() => core.transfers.pause(id)).toThrow('pause failed');
    expect(core.calls).toEqual(
      expect.arrayContaining([
        { method: 'files.list', args: [hostId, null] },
        {
          method: 'files.download',
          args: [hostId, '/Users/demo/notes-1020.md'],
        },
        { method: 'transfers.pause', args: [id] },
      ]),
    );
  });
});
