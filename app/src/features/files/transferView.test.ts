import { FIXTURE_NOW, sshServers } from '../../native/rscore/fixtures';
import {
  srcGpu,
  srcNas,
  srcOffice,
  srcStudio,
  transferFixtures,
} from './fakeFiles';
import { sourceKey, type FileSource, type Transfer } from './filesApi';
import {
  fileIcon,
  formatEta,
  formatProgress,
  formatSpeed,
  groupTransfers,
  percent,
  phoneLabel,
  transferRow,
} from './transferView';
import type { TransferNames } from './transferView';

const TZ = 480;

const NAMES: Record<string, string> = {
  'computer:h7v2m9q3k8x4tb1a': 'Studio Mac mini',
  'computer:w3k8p2x7m4q9tb6c': '工位 Windows',
};

const names: TransferNames = {
  source: (source: FileSource) => {
    const name = NAMES[sourceKey(source)];
    if (name !== undefined) {
      return name;
    }
    if (source.kind === 'server') {
      return (
        sshServers.find(server => server.serverId === source.serverId)?.name ??
        '?'
      );
    }
    return '?';
  },
  phone: 'iPhone',
};

function fixture(id: string): Transfer {
  const transfer = transferFixtures.find(item => item.transferId === id);
  if (transfer === undefined) {
    throw new Error(`missing transfer fixture: ${id}`);
  }
  return transfer;
}

function changed(id: string, patch: Partial<Transfer>): Transfer {
  return { ...fixture(id), ...patch };
}

describe('transferView', () => {
  test('formats progress in the size unit with a rounded percentage', () => {
    expect(formatProgress(19084083, 29779558)).toBe('18.2 / 28.4 MB · 64%');
    expect(formatProgress(43306189, 196712858)).toBe('41.3 / 187.6 MB · 22%');
    expect(formatProgress(0, 0)).toMatch(/100%$/);
    expect(percent(2841120870, 4509715661)).toBe(63);
  });

  test('formats speed and eta', () => {
    expect(formatSpeed(5033165, 29779558 - 19084083)).toBe(
      '4.8 MB/s · 剩余 2 秒',
    );
    expect(formatSpeed(90177536, 196712858 - 43306189)).toBe(
      '86 MB/s · 剩余 2 秒',
    );
    expect(formatSpeed(1048576, 600 * 1048576)).toMatch(/剩余 10 分钟$/);
    expect(formatSpeed(0, 10)).toBe('正在连接…');
    expect(formatEta(2)).toBe('2 秒');
    expect(formatEta(90)).toBe('2 分钟');
    expect(formatEta(3720)).toBe('1 小时 2 分');
  });

  test('builds the upload row shown on the Files canvas', () => {
    const row = transferRow(fixture('t-img'), names, TZ);
    expect(row.route).toBe('iPhone → 工位 Windows › 下载');
    expect(row.pathMark).toBe('tailnetDerp');
    expect(row.pathLabel).toBe('中继');
    expect(row.action).toBe('pause');
    expect(row.icon).toBe('screenshot');
    expect(row.progressText).toBe('18.2 / 28.4 MB · 64%');
    expect(row.speedText).toBe('4.8 MB/s · 剩余 2 秒');
    expect(row.a11y).toBe(
      'IMG_4021.HEIC，iPhone 到 工位 Windows，下载，中继，已传 64%，剩余 2 秒',
    );
  });

  test('builds the active download row shown on the Files canvas', () => {
    const row = transferRow(fixture('t-dmg'), names, TZ);
    expect(row.route).toBe('Studio Mac mini › 桌面');
    expect(row.icon).toBe('file');
    expect(row.progressText).toBe('41.3 / 187.6 MB · 22%');
    expect(row.speedText).toBe('86 MB/s · 剩余 2 秒');
    expect(row.pathLabel).toBe('直连');
  });

  test('builds interrupted and failed rows', () => {
    const interrupted = transferRow(fixture('t-tar'), names, TZ);
    expect(interrupted.route).toBe('nas-home → iPhone · 中断于 63%');
    expect(interrupted.action).toBe('resume');
    expect(interrupted.tone).toBe('relay');
    expect(interrupted.pathMark).toBeNull();

    const failed = transferRow(
      changed('t-tar', { state: 'failed', fail: 'sourceChanged' }),
      names,
      TZ,
    );
    expect(failed.route).toMatch(/源文件改过了，要重新传$/);
    expect(failed.route).toBe('nas-home → iPhone · 源文件改过了，要重新传');
    expect(failed.action).toBe('retry');
    expect(failed.tone).toBe('bad');
    expect(failed.pathLabel).toBeNull();
  });

  test('builds completed rows with destination, size and time', () => {
    const png = transferRow(fixture('t-png'), names, TZ);
    expect(png.doneText).toBe('Studio Mac mini → 相册 · 2.4 MB');
    expect(png.time).toBe('10:21');
    expect(png.action).toBeNull();
    expect(png.pathMark).toBeNull();

    const log = transferRow(fixture('t-log'), names, TZ);
    expect(log.doneText).toBe('gpu-train-02 → 文件 App · 4.1 MB');

    const xlsx = transferRow(fixture('t-xlsx'), names, TZ);
    expect(xlsx.doneText).toBe('iPhone → 工位 Windows · 860 KB');
  });

  test('paused rows resume and expose paused speed', () => {
    const row = transferRow(
      changed('t-img', { state: 'paused', bytesPerSec: 0 }),
      names,
      TZ,
    );
    expect(row.speedText).toBe('已暂停');
    expect(row.action).toBe('resume');
    expect(row.progressText).toBe('18.2 / 28.4 MB · 64%');
    expect(row.a11y).toBe(
      'IMG_4021.HEIC，iPhone 到 工位 Windows，下载，中继，已传 64%，已暂停',
    );
  });

  test('groups active, stalled and completed transfers', () => {
    expect(fixture('t-tar').startedAt).toBe(fixture('t-tar').updatedAt);
    expect(fixture('t-png').startedAt).toBe(fixture('t-png').updatedAt);
    expect(fixture('t-log').startedAt).toBe(fixture('t-log').updatedAt);
    expect(fixture('t-xlsx').startedAt).toBe(fixture('t-xlsx').updatedAt);
    const groups = groupTransfers(transferFixtures, FIXTURE_NOW, 480);
    expect(groups.active.map(item => item.transferId)).toEqual([
      't-img',
      't-dmg',
    ]);
    expect(groups.stalled.map(item => item.transferId)).toEqual(['t-tar']);
    expect(groups.doneToday.map(item => item.transferId)).toEqual([
      't-png',
      't-log',
      't-xlsx',
    ]);
    expect(groups.doneEarlier).toEqual([]);

    const old = changed('t-png', {
      transferId: 't-old',
      updatedAt: FIXTURE_NOW - 86400e6,
    });
    const cancelled = changed('t-log', {
      transferId: 't-cancelled',
      state: 'cancelled',
    });
    const withOld = groupTransfers(
      [...transferFixtures, old, cancelled],
      FIXTURE_NOW,
      480,
    );
    expect(withOld.doneEarlier.map(item => item.transferId)).toEqual(['t-old']);
    expect(
      [
        ...withOld.active,
        ...withOld.stalled,
        ...withOld.doneToday,
        ...withOld.doneEarlier,
      ].some(item => item.transferId === 't-cancelled'),
    ).toBe(false);
  });

  test('labels phones and detects screenshot extensions', () => {
    expect(phoneLabel({ platform: 'ios', isPad: false })).toBe('iPhone');
    expect(phoneLabel({ platform: 'ipados', isPad: true })).toBe('iPad');
    expect(phoneLabel({ platform: 'android', isPad: false })).toBe('手机');
    expect(fileIcon('A.HEIC')).toBe('screenshot');
    expect(fileIcon('.PNG')).toBe('screenshot');
    expect(fileIcon('photo.jpeg')).toBe('screenshot');
    expect(fileIcon('notes.md')).toBe('file');
    expect(fileIcon('archive')).toBe('file');
  });

  test('uses the matching source and phone names for server rows', () => {
    expect(names.source(srcGpu)).toBe('gpu-train-02');
    expect(names.source(srcNas)).toBe('nas-home');
    expect(names.source(srcStudio)).toBe('Studio Mac mini');
    expect(names.source(srcOffice)).toBe('工位 Windows');
  });
});
