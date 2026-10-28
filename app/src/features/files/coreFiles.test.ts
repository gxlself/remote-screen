import { createFakeCore } from '../../native/rscore/fakeCore';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { computers, sshServers } from '../../native/rscore/fixtures';
import { createCoreFiles } from './coreFiles';
import { isUnsupported, type FileSource } from './filesApi';

const computerId = computers[0]?.hostId ?? '';
const serverId = sshServers[0]?.serverId ?? '';
const computer: FileSource = { kind: 'computer', hostId: computerId };
const server: FileSource = { kind: 'server', serverId };

async function advanceToDone(): Promise<void> {
  for (let index = 0; index < 12; index += 1) {
    jest.advanceTimersByTime(100);
    await Promise.resolve();
  }
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.clearAllTimers();
  jest.useRealTimers();
});

it('routes listings to the matching core file API', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);

  const computerListing = await files.list(computer, null);
  expect(computerListing.entries).toEqual(
    (await core.files.list(computerId, null)).entries,
  );
  expect(core.calls).toContainEqual({
    method: 'files.list',
    args: [computerId, null],
  });

  const serverListing = await files.list(server, null);
  expect(serverListing.entries).toEqual(
    (await core.sftp.list(serverId, null)).entries,
  );
  expect(core.calls).toContainEqual({
    method: 'sftp.list',
    args: [serverId, null],
  });
  expect(computerListing).not.toHaveProperty('parent');
  expect(computerListing).not.toHaveProperty('truncated');
  files.dispose();
});

it('streams a download to completion and notifies subscribers', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  const states: string[] = [];
  const callback = jest.fn();
  files.subscribe(() => {
    callback();
    const transfer = files.transfers()[0];
    if (transfer !== undefined) {
      states.push(transfer.state);
    }
  });

  const id = await files.download(computer, '/Users/demo/notes-1020.md');
  expect(core.calls).toContainEqual({
    method: 'files.download',
    args: [computerId, '/Users/demo/notes-1020.md'],
  });
  expect(files.transfers()).toHaveLength(1);
  expect(files.transfers()[0]).toMatchObject({
    transferId: id,
    state: 'queued',
    source: computer,
    localUri: null,
  });

  await advanceToDone();

  expect(callback).toHaveBeenCalled();
  expect([...new Set(states)]).toEqual([
    'queued',
    'running',
    'verifying',
    'completed',
  ]);
  expect(files.transfers()[0]).toMatchObject({
    state: 'completed',
    localUri: 'file:///fake/files/notes-1020.md',
    savedTo: null,
  });
  files.dispose();
});

it('returns the same cache reference until a transfer changes', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  await files.download(computer, '/Users/demo/notes-1020.md');

  const first = files.transfers();
  const second = files.transfers();
  expect(second).toBe(first);

  jest.advanceTimersByTime(0);
  await Promise.resolve();
  expect(files.transfers()).not.toBe(first);
  files.dispose();
});

it('decodes file URIs before uploading and rejects non-file URIs', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);

  await files.upload(computer, 'file:///var/tmp/Inbox/a%20b.txt', null);
  expect(core.calls).toContainEqual({
    method: 'files.upload',
    args: [computerId, '/var/tmp/Inbox/a b.txt', null],
  });

  const callCount = core.calls.length;
  const error = await files
    .upload(computer, 'content://x', null)
    .catch(caught => caught);
  expect(error).toBeInstanceOf(RsCoreError);
  expect(error).toMatchObject({ kind: 'internal' });
  expect(core.calls).toHaveLength(callCount);
  files.dispose();
});

it('forwards pause, resume and cancel to the core transfer API', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  const id = await files.download(computer, '/Users/demo/notes-1020.md');

  files.pause(id);
  expect(core.calls).toContainEqual({
    method: 'transfers.pause',
    args: [id],
  });
  await files.resume(id);
  expect(core.calls).toContainEqual({
    method: 'transfers.resume',
    args: [id],
  });
  files.cancel(id);
  expect(core.calls).toContainEqual({
    method: 'transfers.cancel',
    args: [id],
  });
  files.dispose();
});

it('logs but does not throw when pause fails', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  const id = await core.files.download(computerId, '/Users/demo/notes-1020.md');
  core.failNext('transfers.pause', new Error('pause failed'));

  expect(() => files.pause(id)).not.toThrow();
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['warn', 'files', expect.stringContaining('pause failed')],
  });
  files.dispose();
});

it('drops transfers after a file.transferRemoved event', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  const id = await files.download(computer, '/Users/demo/notes-1020.md');
  expect(files.transfers().map(item => item.transferId)).toContain(id);

  core.transfers.remove(id);
  expect(files.transfers().some(item => item.transferId === id)).toBe(false);
  files.dispose();
});

it('translates RsCoreError messages while preserving kind and status', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  core.failNext(
    'files.list',
    new RsCoreError('http', 'files.notFound: /x', 404),
  );

  const error = await files.list(computer, '/x').catch(caught => caught);
  expect(error).toMatchObject({
    kind: 'http',
    httpStatus: 404,
    message: '找不到这个文件或文件夹',
  });
  files.dispose();
});

it('keeps isUnsupported true after translating unsupported errors', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  core.failNext(
    'files.list',
    new RsCoreError('unsupported', 'files.unsupported: host'),
  );

  const error = await files.list(computer, null).catch(caught => caught);
  expect(isUnsupported(error)).toBe(true);
  expect(error).toMatchObject({
    message: '这台电脑的版本太旧，升级后才能传文件',
  });
  files.dispose();
});

it('stops emitting subscriber callbacks after dispose', async () => {
  const core = createFakeCore();
  const files = createCoreFiles(core);
  const callback = jest.fn();
  files.subscribe(callback);
  await files.download(computer, '/Users/demo/notes-1020.md');
  const transfer = core.transfers.list()[0];
  expect(transfer).toBeDefined();
  callback.mockClear();

  files.dispose();
  if (transfer !== undefined) {
    core.emit({ type: 'file.transfer', transfer });
  }

  expect(callback).not.toHaveBeenCalled();
  expect(core.subscriberCount()).toBe(0);
});
