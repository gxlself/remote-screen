import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { fpDisplay } from '../../features/format/fingerprint';
import { clearPairFiles } from '../../features/pairing/pairFileCache';
import { createFakeCore, type FakeCore } from '../../native/rscore/fakeCore';
import { pairFileSummary, pairFileText } from '../../native/rscore/fixtures';
import type { PairFileSummary } from '../../native/rscore/types';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairFileScreen from '.';

const FILE_URI = 'file:///private/var/mobile/Inbox/Studio-20261020-0258.rspair';
const SECRET = 'fixture-only';
type PairFileNavigation = ReturnType<typeof mockNavigation<'PairFile'>>;
type LoadFileMock = jest.SpiedFunction<FakeCore['pairing']['loadFile']>;

function props(params?: RootScreenProps<'PairFile'>['route']['params']) {
  const navigation = mockNavigation<'PairFile'>();
  return {
    navigation,
    route: mockRoute('PairFile', params),
  } as RootScreenProps<'PairFile'> & { navigation: PairFileNavigation };
}

async function settle() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function renderPairFile(
  params?: RootScreenProps<'PairFile'>['route']['params'],
  opts?: {
    pairFiles?: Record<string, PairFileSummary>;
    platform?: FakePlatform;
    prepare?: (core: ReturnType<typeof createFakeCore>) => void;
  },
) {
  const core = createFakeCore(
    opts?.pairFiles === undefined ? undefined : { pairFiles: opts.pairFiles },
  );
  opts?.prepare?.(core);
  const screenProps = props(params);
  const view = await renderWithProviders(<PairFileScreen {...screenProps} />, {
    core,
    platform: opts?.platform,
  });
  await settle();
  return { ...view, ...screenProps, core };
}

async function enterAndRead(text: string) {
  await fireEvent.changeText(screen.getByTestId('pair-file-input'), text);
  await fireEvent.press(screen.getByTestId('pair-file-read'));
  await settle();
}

afterEach(async () => {
  await cleanup();
  clearPairFiles();
  jest.restoreAllMocks();
});

describe('PairFile', () => {
  it('reads pasted text, shows the summary and pairs with only the fileId', async () => {
    const { core, navigation } = await renderPairFile();
    const loadFile = jest.spyOn(core.pairing, 'loadFile');

    expect(screen.getByText('导入配对文件')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '在「文件」App 或 AirDrop 里点电脑导出的 .rspair 文件，或者把文件内容粘贴到下面。',
      ),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('pair-file-input').props).toMatchObject({
      autoCapitalize: 'none',
      autoCorrect: false,
    });

    await enterAndRead(`  ${pairFileText}  `);

    expect(loadFile).toHaveBeenCalledWith({ text: pairFileText });
    expect(screen.getByText(pairFileSummary.hostName)).toBeOnTheScreen();
    expect(
      screen.getByText(
        fpDisplay(pairFileSummary.fp).split(' ').slice(0, 2).join(' '),
      ),
    ).toBeOnTheScreen();
    expect(screen.getByText(pairFileSummary.addrs[0])).toBeOnTheScreen();
    expect(screen.getByText('10月20日 03:08 前有效')).toBeOnTheScreen();
    expect(screen.getByTestId('pair-file-input').props.value).toBe('');

    await fireEvent.press(screen.getByTestId('pair-file-pair'));
    expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
      fileId: pairFileSummary.fileId,
    });
    const calls = navigation as unknown as Record<string, jest.Mock>;
    const navigationCalls = JSON.stringify([
      calls.replace.mock.calls,
      calls.navigate.mock.calls,
      calls.setParams.mock.calls,
    ]);
    expect(navigationCalls).not.toContain(SECRET);
    expect(navigationCalls).not.toContain(pairFileText);
  });

  it('pastes clipboard text into the input', async () => {
    const platform = new FakePlatform();
    platform.clipboardText = pairFileText;
    const { core } = await renderPairFile(undefined, { platform });
    const loadFile = jest.spyOn(core.pairing, 'loadFile');

    await fireEvent.press(screen.getByTestId('pair-file-paste'));
    await settle();
    expect(platform.calls).toContainEqual({ method: 'readText', args: [] });
    expect(screen.getByTestId('pair-file-input').props.value).toBe(
      pairFileText,
    );

    await fireEvent.press(screen.getByTestId('pair-file-read'));
    await settle();
    expect(loadFile).toHaveBeenCalledWith({ text: pairFileText });
  });

  it('reports an empty clipboard without clearing the input', async () => {
    const platform = new FakePlatform();
    const { core } = await renderPairFile(undefined, { platform });
    await fireEvent.changeText(screen.getByTestId('pair-file-input'), 'draft');

    await fireEvent.press(screen.getByTestId('pair-file-paste'));
    await settle();

    expect(screen.getByText('剪贴板是空的')).toBeOnTheScreen();
    expect(screen.getByTestId('pair-file-input').props.value).toBe('draft');
    expect(core.calls.some(call => call.method === 'pairing.loadFile')).toBe(
      false,
    );
  });

  it('auto-loads a text route parameter and clears it from navigation', async () => {
    let loadFile: LoadFileMock | undefined;
    const { navigation } = await renderPairFile(
      { text: pairFileText },
      {
        prepare: core => {
          loadFile = jest.spyOn(core.pairing, 'loadFile');
        },
      },
    );

    expect(loadFile).toHaveBeenCalledWith({ text: pairFileText });
    expect(navigation.setParams).toHaveBeenCalledWith({
      text: undefined,
      uri: undefined,
    });
    expect(navigation.replace).not.toHaveBeenCalled();
    expect(screen.getByText(pairFileSummary.hostName)).toBeOnTheScreen();
  });

  it('auto-loads a file URI and clears it from navigation', async () => {
    let loadFile: LoadFileMock | undefined;
    const { navigation } = await renderPairFile(
      { uri: FILE_URI },
      {
        pairFiles: { [FILE_URI]: pairFileSummary },
        prepare: core => {
          loadFile = jest.spyOn(core.pairing, 'loadFile');
        },
      },
    );

    expect(loadFile).toHaveBeenCalledWith({ uri: FILE_URI });
    expect(navigation.setParams).toHaveBeenCalledWith({
      text: undefined,
      uri: undefined,
    });
    expect(navigation.replace).not.toHaveBeenCalled();
  });

  it('explains that a content URI cannot be read directly', async () => {
    await renderPairFile({ uri: 'content://x/y.rspair' });

    expect(
      screen.getByText(
        '没法直接读取这个文件，请在文件里长按 › 拷贝内容后粘贴到这里',
      ),
    ).toBeOnTheScreen();
  });

  it('rejects arbitrary text and keeps it editable', async () => {
    await renderPairFile();
    await enterAndRead('hello');

    expect(
      screen.getByText('这不是 Remote Screen 的配对文件'),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('pair-file-input').props.value).toBe('hello');
    expect(screen.queryByTestId('pair-file-summary')).toBeNull();
  });

  it('marks an expired summary and disables pairing', async () => {
    const expired = { ...pairFileSummary, expired: true };
    await renderPairFile(undefined, {
      pairFiles: { [pairFileText]: expired },
    });
    await enterAndRead(pairFileText);

    expect(screen.getByText('已过期')).toBeOnTheScreen();
    expect(screen.getByText('在电脑上重新导出一份')).toBeOnTheScreen();
    expect(screen.getByTestId('pair-file-pair')).toBeDisabled();
  });

  it('offers a retry for other failures', async () => {
    const { core, navigation } = await renderPairFile();
    core.failNext('pairing.loadFile', new Error('disk unavailable'));
    await enterAndRead(pairFileText);

    expect(
      screen.getByText('读不了这个文件，请检查内容后再试'),
    ).toBeOnTheScreen();
    expect(navigation.setParams).not.toHaveBeenCalled();
    expect(screen.getByTestId('pair-file-input').props.value).toBe(
      pairFileText,
    );
  });
});
