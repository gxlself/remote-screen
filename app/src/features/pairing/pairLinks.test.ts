import { classifyPairLink, isContentUrl } from './pairLinks';

const PAIRFILE = 'rsp://pairfile?v=1&h=h1&fp=fp&f=file1&s=secret&n=Mac';

describe('classifyPairLink', () => {
  it.each([PAIRFILE, 'RSP://PairFile?v=1&h=h1', 'rsp://PAIRFILE?v=1'])(
    'accepts a pairing-file URI: %s',
    url => {
      expect(classifyPairLink(url)).toEqual({ text: url });
    },
  );

  it.each([
    'file:///private/var/mobile/Containers/Data/Application/Inbox/Studio-20261020-0258.rspair',
    'file:///tmp/Studio%20Mac.RSPAIR',
    'file:///tmp/Studio%20Mac.rspair?x=1#part',
    'FILE:///tmp/Studio.rspair',
  ])('accepts an rspair file URL: %s', url => {
    expect(classifyPairLink(url)).toEqual({ uri: url });
  });

  it('keeps a malformed escape as a file link when the raw suffix matches', () => {
    expect(classifyPairLink('file:///%E0%A4%A.rspair')).toEqual({
      uri: 'file:///%E0%A4%A.rspair',
    });
  });

  it.each([
    'file:///a.txt',
    'file:///a.rspair.txt',
    'rsp://pair?v=1',
    'https://hs.example.com/x.rspair',
    'content://x/y.rspair',
    'rsp://pairfile',
    'rsp://pairfilex?v=1',
    '',
  ])('rejects a non pairing-file link: %s', url => {
    expect(classifyPairLink(url)).toBeNull();
  });

  it('rejects a link over 8192 bytes', () => {
    const prefix = 'file:///';
    const url =
      prefix + 'a'.repeat(8192 - prefix.length - '.rspair'.length) + '.rspair';
    expect(url).toHaveLength(8192);
    expect(classifyPairLink(url)).toEqual({ uri: url });
    expect(classifyPairLink(`${url}a`)).toBeNull();
  });
});

describe('isContentUrl', () => {
  it.each([
    'content://com.android.providers.downloads.documents/document/123',
    'CONTENT://provider/document/123',
  ])('recognizes an Android content URI: %s', url => {
    expect(isContentUrl(url)).toBe(true);
  });

  it.each(['file:///tmp/report.pdf', 'rsp://pairfile?v=1'])(
    'rejects a non-content URL: %s',
    url => {
      expect(isContentUrl(url)).toBe(false);
    },
  );
});
