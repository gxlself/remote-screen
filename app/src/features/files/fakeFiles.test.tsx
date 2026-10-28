import { Text } from 'react-native';
import type { JSX } from 'react';
import { act, screen } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import { FilesProvider, useFiles } from './FilesContext';
import { useTransfers } from './useTransfers';
import {
  createFakeFiles,
  srcOffice,
  srcStudio,
  transferFixtures,
} from './fakeFiles';
import {
  UNSUPPORTED_FILES,
  UNSUPPORTED_PICKER,
  isUnsupported,
} from './filesApi';

function FilesProbe(): JSX.Element {
  const { files, picker, available } = useFiles();
  const transfers = useTransfers();
  return (
    <>
      <Text>{available ? 'available' : 'unavailable'}</Text>
      <Text testID="transfer-count">{transfers.length}</Text>
      <Text testID="img-state">
        {transfers.find(item => item.transferId === 't-img')?.state ?? 'none'}
      </Text>
      <Text testID="snapshot-count">{files.transfers().length}</Text>
      <Text>{picker.pickFiles === undefined ? 'no-picker' : 'picker'}</Text>
    </>
  );
}

describe('FakeFiles and FilesContext', () => {
  test('falls back to the unsupported API without a provider', async () => {
    await renderWithProviders(<FilesProbe />);
    expect(await screen.findByText('unavailable')).toBeTruthy();

    const listError = await UNSUPPORTED_FILES.list(srcStudio, null).catch(
      error => error,
    );
    expect(isUnsupported(listError)).toBe(true);
    expect(UNSUPPORTED_FILES.transfers()).toEqual([]);
    const pickerError = await UNSUPPORTED_PICKER.pickFiles().catch(
      error => error,
    );
    expect(isUnsupported(pickerError)).toBe(true);
  });

  test('exposes transfer updates and unsubscribes on unmount', async () => {
    const fake = createFakeFiles();
    const rendered = await renderWithProviders(
      <FilesProvider files={fake} picker={fake}>
        <FilesProbe />
      </FilesProvider>,
    );
    expect(screen.getByTestId('transfer-count')).toHaveTextContent('6');
    expect(screen.getByTestId('img-state')).toHaveTextContent('running');
    await act(() => fake.pause('t-img'));
    expect(screen.getByTestId('img-state')).toHaveTextContent('paused');
    expect(fake.subscriberCount()).toBe(1);
    await rendered.unmount();
    expect(fake.subscriberCount()).toBe(0);
  });

  test('lists fixture directories and distinguishes unsupported from 404', async () => {
    const fake = createFakeFiles();
    const home = await fake.list(srcStudio, null);
    expect(home.path).toBe('/Users/alice');
    expect(home.entries.some(entry => entry.name === '桌面')).toBe(true);

    const desktop = await fake.list(srcStudio, '/Users/alice/桌面');
    expect(desktop.entries).toHaveLength(5);

    await expect(fake.list(srcOffice, null)).rejects.toMatchObject({
      kind: 'unsupported',
    });
    const notFound = fake.list(srcStudio, '/nope');
    await expect(notFound).rejects.toMatchObject({
      kind: 'http',
      httpStatus: 404,
    });
  });

  test('returns deep copies of listings', async () => {
    const fake = createFakeFiles();
    const first = await fake.list(srcStudio, null);
    const entry = first.entries[0];
    if (entry === undefined) {
      throw new Error('missing fixture entry');
    }
    entry.name = 'changed';
    const second = await fake.list(srcStudio, null);
    expect(second.entries[0]?.name).toBe('桌面');
  });

  test('records download calls, ids and notifications', async () => {
    const fake = createFakeFiles();
    let notified = 0;
    fake.subscribe(() => {
      notified += 1;
    });
    const remotePath = '/Users/alice/桌面/notes-1020.md';
    await expect(fake.download(srcStudio, remotePath)).resolves.toBe('t-dl-1');
    const added = fake.transfers().find(item => item.transferId === 't-dl-1');
    expect(added).toMatchObject({
      name: 'notes-1020.md',
      state: 'queued',
      direction: 'download',
      remotePath,
    });
    expect(notified).toBe(1);
    expect(fake.calls[fake.calls.length - 1]).toEqual({
      method: 'download',
      args: [srcStudio, remotePath],
    });
  });

  test('uploads decode the local file name and resume stalled rows', async () => {
    const fake = createFakeFiles();
    await expect(
      fake.upload(srcOffice, 'file:///fake/a%20b.pdf', null),
    ).resolves.toBe('t-up-1');
    const uploaded = fake
      .transfers()
      .find(item => item.transferId === 't-up-1');
    expect(uploaded).toMatchObject({
      name: 'a b.pdf',
      direction: 'upload',
      state: 'queued',
    });

    await fake.resume('t-tar');
    expect(
      fake.transfers().find(item => item.transferId === 't-tar')?.state,
    ).toBe('running');
  });

  test('injects one failure per failNext call', async () => {
    const fake = createFakeFiles();
    fake.failNext('download', new Error('x'));
    await expect(
      fake.download(srcStudio, '/Users/alice/桌面/notes-1020.md'),
    ).rejects.toThrow('x');
    await expect(
      fake.download(srcStudio, '/Users/alice/桌面/notes-1020.md'),
    ).resolves.toBe('t-dl-1');
  });

  test('keeps no-op mutations quiet and emits additions or updates', async () => {
    const fake = createFakeFiles();
    let notified = 0;
    fake.subscribe(() => {
      notified += 1;
    });
    fake.pause('missing');
    await fake.resume('missing');
    fake.cancel('missing');
    expect(notified).toBe(0);

    const extra = { ...transferFixtures[0] };
    extra.transferId = 't-extra';
    fake.emit(extra);
    expect(notified).toBe(1);
    expect(fake.transfers().some(item => item.transferId === 't-extra')).toBe(
      true,
    );

    fake.emit({ ...extra, state: 'paused' });
    expect(notified).toBe(2);
    expect(
      fake.transfers().find(item => item.transferId === 't-extra')?.state,
    ).toBe('paused');
  });

  test('returns picked files and reports subscribers', async () => {
    const picked = [{ uri: 'file:///fake/a.pdf', name: 'a.pdf', size: 1 }];
    const fake = createFakeFiles({ picked });
    await expect(fake.pickFiles()).resolves.toEqual(picked);
    fake.picked.push({ uri: 'file:///fake/b.pdf', name: 'b.pdf', size: 2 });
    const first = await fake.pickFiles();
    first.push({ uri: 'file:///fake/c.pdf', name: 'c.pdf', size: 3 });
    expect(await fake.pickFiles()).toHaveLength(2);
    expect(fake.subscriberCount()).toBe(0);
    expect(await fake.saveToFiles('file:///fake/a.pdf', 'a.pdf')).toBe(true);
  });

  test('keeps fixtures independent from each other', async () => {
    const first = createFakeFiles();
    const second = createFakeFiles();
    expect(first.transfers()).toHaveLength(transferFixtures.length);
    expect(second.transfers()).toHaveLength(6);
    expect(first).not.toBe(second);
    const firstHome = await first.list(srcStudio, null);
    const entry = firstHome.entries[0];
    if (entry === undefined) {
      throw new Error('missing fixture entry');
    }
    entry.name = 'changed';
    expect((await second.list(srcStudio, null)).entries[0]?.name).toBe('桌面');
  });
});
