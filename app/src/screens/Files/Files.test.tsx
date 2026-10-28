import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { FilesProvider } from '../../features/files/FilesContext';
import {
  createFakeFiles,
  srcOffice,
  srcStudio,
} from '../../features/files/fakeFiles';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import FilesScreen from './index';

async function tick(ms = 1) {
  await act(() => jest.advanceTimersByTimeAsync(ms));
}

function screenUi() {
  return (
    <FilesScreen
      navigation={
        mockNavigation() as unknown as TabScreenProps<'Files'>['navigation']
      }
      route={
        {
          key: 'Files-test',
          name: 'Files',
          params: undefined,
        } as TabScreenProps<'Files'>['route']
      }
    />
  );
}

async function renderFiles(
  fake = createFakeFiles(),
  platform = new FakePlatform(),
) {
  const ui = (
    <FilesProvider files={fake} picker={fake}>
      {screenUi()}
    </FilesProvider>
  );
  // Online paired Hosts are the computers that can take files.
  const result = await renderWithProviders(ui, {
    platform,
    localDirectory: {},
  });
  return { ...result, fake, platform };
}

beforeEach(() => {
  jest.useFakeTimers({ now: FIXTURE_NOW / 1000 });
});

afterEach(() => {
  cleanup();
  jest.useRealTimers();
});

it('shows transfers, progress and today completed rows', async () => {
  const { fake } = await renderFiles();

  expect(screen.getByText('文件')).toBeOnTheScreen();
  expect(screen.getByText('发送文件')).toBeOnTheScreen();
  expect(
    screen.getByRole('radio', { name: '传输' }).props.accessibilityState,
  ).toEqual({ selected: true });
  expect(screen.getByText('进行中 · 2')).toBeOnTheScreen();
  expect(screen.getByText('IMG_4021.HEIC')).toBeOnTheScreen();
  expect(screen.getByText('18.2 / 28.4 MB · 64%')).toBeOnTheScreen();
  expect(screen.getByText('4.8 MB/s · 剩余 2 秒')).toBeOnTheScreen();
  expect(screen.getByText('41.3 / 187.6 MB · 22%')).toBeOnTheScreen();
  expect(screen.getByText('nas-home → iPhone · 中断于 63%')).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '续传 dataset-1019.tar' }),
  ).toBeOnTheScreen();
  expect(screen.getByText('今天已完成')).toBeOnTheScreen();
  expect(
    await screen.findByText('Studio Mac mini → 相册 · 2.4 MB'),
  ).toBeOnTheScreen();
  expect(
    screen.getByText('gpu-train-02 → 文件 App · 4.1 MB'),
  ).toBeOnTheScreen();
  expect(screen.getByText('iPhone → 工位 Windows · 860 KB')).toBeOnTheScreen();

  const progress = screen.getByLabelText('IMG_4021.HEIC 传输进度');
  expect(progress.props.accessibilityValue.now).toBe(64);
  expect(fake.calls.some(call => call.method === 'pause')).toBe(false);
});

it('pauses and resumes an active transfer, and resumes a stalled one', async () => {
  const { fake } = await renderFiles();

  await fireEvent.press(screen.getByLabelText('暂停 IMG_4021.HEIC'));
  expect(fake.calls).toContainEqual({ method: 'pause', args: ['t-img'] });
  expect(await screen.findByText('已暂停')).toBeOnTheScreen();
  expect(screen.getByText('继续')).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('继续'));
  await waitFor(() =>
    expect(fake.calls).toContainEqual({ method: 'resume', args: ['t-img'] }),
  );

  await fireEvent.press(
    screen.getByRole('button', { name: '续传 dataset-1019.tar' }),
  );
  await waitFor(() =>
    expect(fake.calls).toContainEqual({ method: 'resume', args: ['t-tar'] }),
  );
});

it('opens a completed download with the system share sheet', async () => {
  const { platform } = await renderFiles();

  await fireEvent.press(screen.getByText('截屏 10.20.png'));

  await waitFor(() =>
    expect(platform.calls).toContainEqual({
      method: 'share',
      args: [
        {
          url: 'file:///fake/截屏 10.20.png',
          title: '截屏 10.20.png',
        },
      ],
    }),
  );
});

it('switches to the BrowseView and back while keeping the location', async () => {
  const { fake } = await renderFiles();

  await fireEvent.press(screen.getByRole('radio', { name: '浏览电脑' }));
  expect(await screen.findByText('Studio Mac mini')).toBeOnTheScreen();
  expect(await screen.findByText('桌面')).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('桌面'));
  await screen.findByText('remote-screen');
  expect(
    fake.calls.some(
      call => call.method === 'list' && call.args[1] === '/Users/alice/桌面',
    ),
  ).toBe(true);

  await fireEvent.press(screen.getByRole('radio', { name: '传输' }));
  expect(screen.getByText('进行中 · 2')).toBeOnTheScreen();
});

it('sends a picked file to a computer and adds its transfer', async () => {
  const fake = createFakeFiles();
  fake.picked = [{ uri: 'file:///fake/a.pdf', name: 'a.pdf', size: 1000 }];
  const { platform } = await renderFiles(fake);

  await fireEvent.press(screen.getByText('发送文件'));
  expect(await screen.findByText('发送 a.pdf')).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '工位 Windows，放进下载文件夹' }),
  ).toBeOnTheScreen();

  await fireEvent.press(
    screen.getByRole('button', {
      name: '工位 Windows，放进下载文件夹',
    }),
  );

  await waitFor(() =>
    expect(fake.calls).toContainEqual({
      method: 'upload',
      args: [srcOffice, 'file:///fake/a.pdf', null],
    }),
  );
  await waitFor(() => expect(screen.queryByText('发送 a.pdf')).toBeNull());
  expect(
    screen.getByRole('radio', { name: '传输' }).props.accessibilityState,
  ).toEqual({ selected: true });
  expect(screen.getByText('已开始发送到 工位 Windows')).toBeOnTheScreen();
  expect(screen.getByText('a.pdf')).toBeOnTheScreen();
  expect(platform.calls.some(call => call.method === 'share')).toBe(false);
});

it('uploads multiple picked files one after another', async () => {
  const fake = createFakeFiles();
  fake.picked = [
    { uri: 'file:///fake/a.pdf', name: 'a.pdf', size: 1000 },
    { uri: 'file:///fake/b.pdf', name: 'b.pdf', size: 2000 },
  ];
  await renderFiles(fake);

  await fireEvent.press(screen.getByText('发送文件'));
  expect(await screen.findByText('发送 2 个文件')).toBeOnTheScreen();
  await fireEvent.press(
    screen.getByRole('button', {
      name: '工位 Windows，放进下载文件夹',
    }),
  );

  await waitFor(() =>
    expect(fake.calls.filter(call => call.method === 'upload')).toHaveLength(2),
  );
  expect(
    fake.calls.filter(call => call.method === 'upload').map(call => call.args),
  ).toEqual([
    [srcOffice, 'file:///fake/a.pdf', null],
    [srcOffice, 'file:///fake/b.pdf', null],
  ]);
  expect(screen.getByText('已开始发送到 工位 Windows')).toBeOnTheScreen();
});

it('sends to the current folder after browsing into it', async () => {
  const fake = createFakeFiles();
  fake.picked = [{ uri: 'file:///fake/b.pdf', name: 'b.pdf', size: 1000 }];
  await renderFiles(fake);

  await fireEvent.press(screen.getByRole('radio', { name: '浏览电脑' }));
  await fireEvent.press(await screen.findByText('桌面'));
  await screen.findByText('remote-screen');

  await fireEvent.press(screen.getByText('发送文件'));
  expect(await screen.findByText('发送 b.pdf')).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: 'Studio Mac mini，当前文件夹 · 桌面' }),
  ).toBeOnTheScreen();

  await fireEvent.press(
    screen.getByRole('button', {
      name: 'Studio Mac mini，当前文件夹 · 桌面',
    }),
  );

  await waitFor(() =>
    expect(fake.calls).toContainEqual({
      method: 'upload',
      args: [srcStudio, 'file:///fake/b.pdf', '/Users/alice/桌面'],
    }),
  );
});

it('does nothing when the picker returns no files', async () => {
  const fake = createFakeFiles();
  fake.picked = [];
  await renderFiles(fake);

  await fireEvent.press(screen.getByText('发送文件'));
  await tick();

  expect(fake.calls).toContainEqual({ method: 'pickFiles', args: [] });
  expect(screen.queryByText(/^发送 /)).toBeNull();
});

it('reports unsupported picking while keeping the page usable', async () => {
  const fake = createFakeFiles();
  fake.failNext('pickFiles', new RsCoreError('unsupported', 'x'));
  await renderFiles(fake);

  await fireEvent.press(screen.getByText('发送文件'));
  expect(
    await screen.findByText('当前版本还不能从手机选文件'),
  ).toBeOnTheScreen();
});

it('keeps the sheet open when uploading fails', async () => {
  const fake = createFakeFiles();
  fake.picked = [{ uri: 'file:///fake/a.pdf', name: 'a.pdf', size: 1000 }];
  fake.failNext('upload', new Error('disk full'));
  await renderFiles(fake);

  await fireEvent.press(screen.getByText('发送文件'));
  await fireEvent.press(
    await screen.findByRole('button', {
      name: '工位 Windows，放进下载文件夹',
    }),
  );

  expect(await screen.findByText('没发出去：disk full')).toBeOnTheScreen();
  expect(screen.getByText('发送 a.pdf')).toBeOnTheScreen();
});

it('shows the empty transfer state and opens browse from it', async () => {
  await renderFiles(createFakeFiles({ transfers: [] }));

  expect(screen.getByText('还没有传过文件')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '在「浏览电脑」里轻点文件下载到手机，或者点右上角「发送文件」。',
    ),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByRole('button', { name: '浏览电脑' }));
  expect(await screen.findByText('Studio Mac mini')).toBeOnTheScreen();
});

it('shows the unsupported state when FilesProvider is absent', async () => {
  await renderWithProviders(screenUi(), { platform: new FakePlatform() });

  expect(screen.getByText('文件传输还没准备好')).toBeOnTheScreen();
  expect(
    screen.getByText('当前版本还不能传文件，更新后再来看看。'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('发送文件')).toBeNull();
  expect(screen.queryByRole('radiogroup', { name: '文件视图' })).toBeNull();
});
