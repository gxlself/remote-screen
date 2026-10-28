import { isIncomingFileUrl, useIncomingShare } from './incomingShare';

beforeEach(() => {
  useIncomingShare.setState({ pending: [] });
});

describe('isIncomingFileUrl', () => {
  it.each(['file:///tmp/report.pdf', 'FILE:///private/var/mobile/report.pdf'])(
    'accepts a regular file URL: %s',
    url => {
      expect(isIncomingFileUrl(url)).toBe(true);
    },
  );

  it.each([
    'rsp://pair?v=1',
    'https://hs.example.com/report.pdf',
    'content://provider/report.pdf',
    'file:///tmp/studio.rspair',
    'file:///tmp/x.RSPAIR',
  ])('rejects a non-incoming URL: %s', url => {
    expect(isIncomingFileUrl(url)).toBe(false);
  });
});

it('takes all pending files and clears the store', () => {
  const first = { uri: 'file:///tmp/a.pdf', name: 'a.pdf', size: 10 };
  const second = { uri: 'file:///tmp/b.pdf', name: 'b.pdf', size: null };
  useIncomingShare.getState().push(first);
  useIncomingShare.getState().push(second);

  expect(useIncomingShare.getState().take()).toEqual([first, second]);
  expect(useIncomingShare.getState().pending).toEqual([]);
  expect(useIncomingShare.getState().take()).toEqual([]);
});
