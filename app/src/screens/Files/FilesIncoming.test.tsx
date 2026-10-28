import { act, cleanup, screen } from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { FilesProvider } from '../../features/files/FilesContext';
import { useIncomingShare } from '../../features/files/incomingShare';
import { createFakeFiles } from '../../features/files/fakeFiles';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import FilesScreen from './index';

beforeEach(() => {
  useIncomingShare.setState({ pending: [] });
});

afterEach(async () => {
  await cleanup();
  useIncomingShare.setState({ pending: [] });
});

function screenUi() {
  return (
    <FilesScreen
      navigation={
        mockNavigation() as unknown as TabScreenProps<'Files'>['navigation']
      }
      route={
        {
          key: 'Files-incoming-test',
          name: 'Files',
          params: undefined,
        } as TabScreenProps<'Files'>['route']
      }
    />
  );
}

it('opens the send sheet for a pending shared file and consumes it', async () => {
  const file = { uri: 'file:///tmp/RsInbox/a.pdf', name: 'a.pdf', size: 12 };
  useIncomingShare.setState({ pending: [file] });
  const fake = createFakeFiles();
  await renderWithProviders(
    <FilesProvider files={fake} picker={fake}>
      {screenUi()}
    </FilesProvider>,
  );

  expect(await screen.findByText('发送 a.pdf')).toBeOnTheScreen();
  expect(useIncomingShare.getState().pending).toEqual([]);
});

it('collects files arriving after the page mounts into the open send sheet', async () => {
  const fake = createFakeFiles();
  await renderWithProviders(
    <FilesProvider files={fake} picker={fake}>
      {screenUi()}
    </FilesProvider>,
  );

  await act(async () => {
    useIncomingShare.getState().push({
      uri: 'file:///tmp/RsInbox/a.pdf',
      name: 'a.pdf',
      size: 12,
    });
  });
  expect(await screen.findByText('发送 a.pdf')).toBeOnTheScreen();

  await act(async () => {
    useIncomingShare.getState().push({
      uri: 'file:///tmp/RsInbox/b.pdf',
      name: 'b.pdf',
      size: null,
    });
  });
  expect(await screen.findByText('发送 2 个文件')).toBeOnTheScreen();
  expect(useIncomingShare.getState().pending).toEqual([]);
});
