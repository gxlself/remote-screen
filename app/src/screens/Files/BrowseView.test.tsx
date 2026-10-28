import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { BrowseView } from './BrowseView';
import {
  createFakeFiles,
  listingFixtures,
  srcOffice,
  srcStudio,
} from '../../features/files/fakeFiles';
import { FilesProvider } from '../../features/files/FilesContext';
import type { BrowseLocation } from '../../features/files/browseView';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import type { LocalDirectorySeed } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';

async function advance(ms = 1) {
  await act(() => jest.advanceTimersByTimeAsync(ms));
}

async function renderBrowse(
  opts: {
    core?: ReturnType<typeof createFakeCore>;
    platform?: FakePlatform;
    fake?: ReturnType<typeof createFakeFiles>;
    /** Online paired Hosts can take files; defaults to the fixtures. */
    localDirectory?: LocalDirectorySeed;
  } = {},
) {
  const fake = opts.fake ?? createFakeFiles();
  const core = opts.core ?? createFakeCore();
  const platform = opts.platform ?? new FakePlatform();
  const onLocationChange = jest.fn<void, [BrowseLocation | null]>();
  const onMessage = jest.fn<void, [string]>();
  const rendered = await renderWithProviders(
    <FilesProvider files={fake} picker={fake}>
      <BrowseView onLocationChange={onLocationChange} onMessage={onMessage} />
    </FilesProvider>,
    { core, platform, localDirectory: opts.localDirectory ?? {} },
  );
  await advance();
  return { ...rendered, fake, core, platform, onLocationChange, onMessage };
}

function listCalls(fake: ReturnType<typeof createFakeFiles>) {
  return fake.calls.filter(call => call.method === 'list');
}

function downloadCalls(fake: ReturnType<typeof createFakeFiles>) {
  return fake.calls.filter(call => call.method === 'download');
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(new Date(FIXTURE_NOW / 1000));
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('shows only online paired computers and SSH servers as chips', async () => {
  await renderBrowse();

  const studio = await screen.findByRole('button', { name: 'Studio Mac mini' });
  expect(studio.props.accessibilityState).toMatchObject({ selected: true });
  expect(
    screen.getByRole('button', { name: '工位 Windows' }),
  ).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: 'gpu-train-02' }),
  ).toBeOnTheScreen();
  expect(screen.queryByText('小明的 MacBook Pro')).not.toBeOnTheScreen();
});

it('starts in the home directory and opens a folder', async () => {
  const { fake, onLocationChange } = await renderBrowse();

  expect(await screen.findByText('桌面')).toBeOnTheScreen();
  expect(screen.getByText('3 项 · 直连')).toBeOnTheScreen();
  expect(onLocationChange).toHaveBeenLastCalledWith({
    source: srcStudio,
    path: '/Users/alice',
    label: 'Studio Mac mini',
  });

  await fireEvent.press(screen.getByText('桌面'));
  await screen.findByText('remote-screen');

  expect(listCalls(fake)).toContainEqual({
    method: 'list',
    args: [srcStudio, '/Users/alice/桌面'],
  });
  expect(screen.getByText('5 项 · 直连')).toBeOnTheScreen();
  expect(screen.getByText('38 项')).toBeOnTheScreen();
  expect(screen.getByText('已在手机')).toBeOnTheScreen();
  expect(screen.getByText('22%')).toBeOnTheScreen();
  expect(onLocationChange).toHaveBeenLastCalledWith({
    source: srcStudio,
    path: '/Users/alice/桌面',
    label: 'Studio Mac mini',
  });
});

it('downloads a file and shows its queued percentage', async () => {
  const { fake, onMessage } = await renderBrowse();

  await fireEvent.press(await screen.findByText('桌面'));
  await fireEvent.press(await screen.findByText('notes-1020.md'));

  expect(downloadCalls(fake)).toContainEqual({
    method: 'download',
    args: [srcStudio, '/Users/alice/桌面/notes-1020.md'],
  });
  expect(onMessage).toHaveBeenCalledWith(
    '开始下载 notes-1020.md，在「传输」里看进度',
  );
  await waitFor(() => expect(screen.getByText('100%')).toBeOnTheScreen());
});

it('shares a completed download without downloading it again', async () => {
  const { fake, platform } = await renderBrowse();

  await fireEvent.press(await screen.findByText('桌面'));
  await fireEvent.press(await screen.findByText('截屏 10.20.png'));
  await waitFor(() =>
    expect(platform.calls).toContainEqual({
      method: 'share',
      args: [{ url: 'file:///fake/截屏 10.20.png', title: '截屏 10.20.png' }],
    }),
  );
  expect(downloadCalls(fake)).toEqual([]);
});

it('does not restart a file that is already downloading', async () => {
  const { fake, onMessage } = await renderBrowse();

  await fireEvent.press(await screen.findByText('桌面'));
  await fireEvent.press(await screen.findByText('build-0.3.1.dmg'));

  expect(onMessage).toHaveBeenCalledWith('build-0.3.1.dmg 正在下载');
  expect(downloadCalls(fake)).toEqual([]);
});

it('goes back up to the home directory', async () => {
  await renderBrowse();

  await fireEvent.press(await screen.findByText('桌面'));
  await screen.findByText('remote-screen');
  await fireEvent.press(screen.getByRole('button', { name: '上一级' }));

  expect(await screen.findByText('桌面')).toBeOnTheScreen();
  expect(screen.getByText('3 项 · 直连')).toBeOnTheScreen();
});

it('keeps the path row visible for an empty folder', async () => {
  const fake = createFakeFiles({
    listings: {
      ...listingFixtures,
      'computer:h7v2m9q3k8x4tb1a|/Users/alice/桌面/remote-screen': {
        path: '/Users/alice/桌面/remote-screen',
        entries: [],
        netPath: 'tailnetDirect',
      },
    },
  });
  await renderBrowse({ fake });

  await fireEvent.press(await screen.findByText('桌面'));
  await fireEvent.press(await screen.findByText('remote-screen'));

  expect(await screen.findByText('这个文件夹是空的')).toBeOnTheScreen();
  expect(screen.getByText('0 项 · 直连')).toBeOnTheScreen();
  expect(screen.getByRole('button', { name: '上一级' })).toBeOnTheScreen();
});

it('explains unsupported browsing and clears the selected location', async () => {
  const { onLocationChange } = await renderBrowse();

  await screen.findByText('桌面');
  await fireEvent.press(screen.getByRole('button', { name: '工位 Windows' }));

  expect(await screen.findByText('这台电脑还不能浏览')).toBeOnTheScreen();
  await waitFor(() => expect(onLocationChange).toHaveBeenLastCalledWith(null));
});

it('retries after a folder listing fails', async () => {
  const fake = createFakeFiles();
  fake.failNext('list', new Error('boom'));
  await renderBrowse({ fake });

  expect(await screen.findByText('打不开这个文件夹')).toBeOnTheScreen();
  expect(screen.getByText('boom')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '重试' }));

  expect(await screen.findByText('桌面')).toBeOnTheScreen();
});

it('shows the no-source state without path controls', async () => {
  const core = createFakeCore({ sshServers: [] });
  await renderBrowse({ core, localDirectory: { computers: [] } });

  expect(await screen.findByText('没有能传文件的设备')).toBeOnTheScreen();
  expect(
    screen.queryByRole('button', { name: '上一级' }),
  ).not.toBeOnTheScreen();
});

it('uses the selected source when switching chips', async () => {
  const fake = createFakeFiles();
  await renderBrowse({ fake });

  await screen.findByText('桌面');
  await fireEvent.press(screen.getByRole('button', { name: '工位 Windows' }));
  await waitFor(() =>
    expect(listCalls(fake)).toContainEqual({
      method: 'list',
      args: [srcOffice, null],
    }),
  );
});
