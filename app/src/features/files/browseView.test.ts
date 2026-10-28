import {
  directory,
  FIXTURE_NOW,
  sshServers,
} from '../../native/rscore/fixtures';
import {
  crumbs,
  dayTime,
  entryRows,
  fileSources,
  joinPath,
  parentPath,
  sendTargets,
  sourceName,
} from './browseView';
import {
  listingFixtures,
  srcGpu,
  srcOffice,
  srcStudio,
  transferFixtures,
} from './fakeFiles';

const NOW = FIXTURE_NOW;
const TZ = 480;
const MIN = 60e6;
const DAY = 86400e6;

describe('fileSources', () => {
  test('lists paired online computers, then SSH servers in directory order', () => {
    expect(fileSources(directory, sshServers).map(chip => chip.label)).toEqual([
      'Studio Mac mini',
      '工位 Windows',
      'prod-api-01',
      'gpu-train-02',
      'nas-home',
      'build-srv',
    ]);
  });

  test('keeps manual servers and drops directory servers that went offline', () => {
    const offlineNas = {
      ...directory,
      servers: directory.servers.map(server =>
        server.node === 'nas-home' ? { ...server, online: false } : server,
      ),
    };
    expect(
      fileSources(offlineNas, sshServers).map(chip => chip.label),
    ).not.toContain('nas-home');
    expect(
      fileSources(offlineNas, sshServers).map(chip => chip.label),
    ).toContain('build-srv');
  });

  test('resolves display names and falls back to ids', () => {
    expect(sourceName(srcStudio, directory, sshServers)).toBe(
      'Studio Mac mini',
    );
    expect(sourceName(srcGpu, directory, sshServers)).toBe('gpu-train-02');
    expect(
      sourceName(
        { kind: 'computer', hostId: 'missing-host' },
        directory,
        sshServers,
      ),
    ).toBe('missing-host');
    expect(
      sourceName(
        { kind: 'server', serverId: 'missing-server' },
        directory,
        sshServers,
      ),
    ).toBe('missing-server');
  });
});

describe('path helpers', () => {
  test('finds parents and joins paths from the root', () => {
    expect(parentPath('/a/b')).toBe('/a');
    expect(parentPath('/a')).toBe('/');
    expect(parentPath('/')).toBe('/');
    expect(joinPath('/a', 'x')).toBe('/a/x');
    expect(joinPath('/', 'x')).toBe('/x');
  });

  test('builds one-level breadcrumbs', () => {
    expect(crumbs('/Users/alice/桌面')).toEqual({
      parent: 'alice',
      current: '桌面',
      canGoUp: true,
    });
    expect(crumbs('/Users')).toEqual({
      parent: '/',
      current: 'Users',
      canGoUp: true,
    });
    expect(crumbs('/')).toEqual({
      parent: null,
      current: '/',
      canGoUp: false,
    });
  });
});

describe('dayTime', () => {
  test('uses today, yesterday, then the calendar day', () => {
    expect(dayTime(NOW - 37 * MIN, NOW, TZ)).toBe('今天 10:21');
    expect(dayTime(NOW - DAY, NOW, TZ)).toBe('昨天 10:58');
    expect(dayTime(NOW - 3 * DAY, NOW, TZ)).toBe('10月17日');
  });
});

describe('entryRows', () => {
  test('orders folders before files and decorates the desktop listing', () => {
    const listing =
      listingFixtures['computer:h7v2m9q3k8x4tb1a|/Users/alice/桌面'];
    if (listing === undefined) {
      throw new Error('missing desktop listing fixture');
    }
    const rows = entryRows(listing, srcStudio, transferFixtures, NOW, TZ);

    expect(rows.map(row => row.name)).toEqual([
      'remote-screen',
      '设计稿',
      '截屏 10.20.png',
      'build-0.3.1.dmg',
      'notes-1020.md',
    ]);
    expect(rows.map(row => row.icon)).toEqual([
      'folder',
      'folder',
      'screenshot',
      'file',
      'file',
    ]);
    expect(rows[0]?.subtitle).toBe('38 项');
    expect(rows[0]?.a11y).toBe('remote-screen，文件夹，38 项');
    expect(rows[2]?.subtitle).toBe('2.4 MB · 今天 10:21');
    expect(rows[2]?.badge).toEqual({ text: '已在手机', tone: 'good' });
    expect(rows[2]?.localUri).toBe('file:///fake/截屏 10.20.png');
    expect(rows[3]?.badge?.text).toBe('22%');
    expect(rows[4]?.badge).toBeNull();
    expect(rows[4]?.subtitle).toBe('6 KB · 今天 09:30');
    expect(rows[4]?.fullPath).toBe('/Users/alice/桌面/notes-1020.md');
  });

  test('omits missing size and time segments and treats links as files', () => {
    const rows = entryRows(
      {
        path: '/a',
        netPath: null,
        entries: [
          {
            name: 'old-link',
            kind: 'symlink',
            size: null,
            mtimeUs: null,
            childCount: null,
          },
          {
            name: 'new-link',
            kind: 'symlink',
            size: 1024,
            mtimeUs: NOW,
            childCount: null,
          },
        ],
      },
      srcStudio,
      [],
      NOW,
      TZ,
    );
    expect(rows.map(row => row.name)).toEqual(['new-link', 'old-link']);
    expect(rows[0]?.subtitle).toBe('1 KB · 今天 10:58');
    expect(rows[1]?.subtitle).toBe('');
    expect(rows.every(row => row.kind === 'file')).toBe(true);
  });
});

describe('sendTargets', () => {
  test('offers the current folder first, then each source default', () => {
    const chips = fileSources(directory, sshServers);
    const targets = sendTargets(chips, {
      source: srcStudio,
      path: '/Users/alice/桌面',
      label: 'Studio Mac mini',
    });

    expect(targets).toHaveLength(chips.length + 1);
    expect(targets[0]).toMatchObject({
      key: 'current',
      source: srcStudio,
      dir: '/Users/alice/桌面',
      subtitle: '当前文件夹 · 桌面',
    });
    expect(
      targets.find(target => target.title === '工位 Windows'),
    ).toMatchObject({ dir: null, subtitle: '放进下载文件夹' });
    expect(
      targets.find(target => target.title === 'gpu-train-02'),
    ).toMatchObject({ dir: null, subtitle: '放进主目录' });
    expect(sendTargets(chips, null)).toHaveLength(chips.length);
  });

  test('uses the current location kind for servers', () => {
    const targets = sendTargets([], {
      source: srcOffice,
      path: '/Users/alice',
      label: '工位 Windows',
    });
    expect(targets[0]).toMatchObject({
      kind: 'computer',
      dir: '/Users/alice',
    });
  });
});
